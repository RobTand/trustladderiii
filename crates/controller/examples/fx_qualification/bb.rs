use super::*;
use std::{
    io::Write,
    process::{Command, Stdio},
};

#[cfg(not(test))]
const CLIENT: &str = "/home/rob/tmp/bazel-wave2/tools/bbrun.py";
#[cfg(test)]
std::thread_local! {
    static CLIENT: std::cell::RefCell<PathBuf> = std::cell::RefCell::new(
        PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tools/ci/bbrun.py"))
    );
}

fn client_path() -> PathBuf {
    #[cfg(not(test))]
    {
        PathBuf::from(CLIENT)
    }
    #[cfg(test)]
    {
        CLIENT.with(|path| path.borrow().clone())
    }
}
const CLIENT_SHA256: &str = "8016e41d97aac7e83f7b5b39c57aea3c030a35d555e35f2929f09ab6076d7aa0";
const HOLD: &str = "/home/rob/tmp/tl3-build/rust-fv/BB-HOLD";

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Build {
    pub invocation_id: String,
    pub client_sha256: String,
    pub base_head: String,
    pub build_commit: String,
    pub overlay_tree: String,
    pub command_exit: i32,
    pub own_verdict: Value,
    pub log: api::BlobRef,
    pub wrapper_result: api::BlobRef,
    pub artifacts: Vec<Value>,
}

fn blob(path: &Path) -> Result<api::BlobRef> {
    let value = identity(path)?;
    Ok(api::BlobRef {
        path: path.into(),
        sha256: value["sha256"].as_str().ok_or("artifact hash")?.into(),
        bytes: value["bytes"].as_u64().ok_or("artifact size")?,
    })
}

pub fn verify_client() -> Result<()> {
    if durable::digest(&durable::read(&client_path())?) != CLIENT_SHA256 {
        return Err("BuildBuddy client sha256 differs from pinned version".into());
    }
    Ok(())
}

fn client(args: &[String], out: &Path) -> Result<String> {
    verify_client()?;
    let output = Command::new("/usr/bin/python3")
        .arg("-I")
        .arg(client_path())
        .args(args)
        .stdin(Stdio::null())
        .output()?;
    // bbrun's evidence guard rejects credential-bearing output before emitting it.
    let mut target = fs::OpenOptions::new().create(true).append(true).open(out)?;
    target.write_all(&output.stdout)?;
    target.write_all(&output.stderr)?;
    target.sync_all()?;
    let raw = String::from_utf8(output.stdout)?;
    if !output.status.success() && !raw.contains("verdict: ") && !raw.contains("still running: ") {
        return Err("BuildBuddy client failed; retained client log".into());
    }
    Ok(raw)
}

fn verdict(raw: &str) -> Option<Value> {
    raw.lines()
        .filter_map(|line| line.strip_prefix("verdict: "))
        .filter_map(|line| serde_json::from_str(line).ok())
        .next_back()
}

fn invocation(raw: &str) -> Result<String> {
    if let Some(id) = verdict(raw).and_then(|v| v["invocationId"].as_str().map(String::from)) {
        return Ok(id);
    }
    raw.lines()
        .find_map(|line| line.strip_prefix("https://app.buildbuddy.io/invocation/"))
        .map(str::trim)
        .filter(|id| id.len() == 36)
        .map(String::from)
        .ok_or_else(|| "uncertain BuildBuddy submission; retain log and never resubmit".into())
}

fn fetch(id: &str, name: &str, root: &Path) -> Result<api::BlobRef> {
    let target = root.join(name);
    let raw = client(
        &[
            "artifacts".into(),
            id.into(),
            "--fetch".into(),
            name.into(),
            "--out".into(),
            target.to_string_lossy().into_owned(),
        ],
        &root.join("fetch.log"),
    )?;
    let metadata: Vec<Value> = raw
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|value| value["name"] == name)
        .collect();
    if metadata.len() != 1 {
        return Err("BuildBuddy artifact name missing or ambiguous".into());
    }
    let reference = blob(&target)?;
    let uri = metadata[0]["uri"]
        .as_str()
        .ok_or("artifact CAS URI missing")?;
    let components: Vec<_> = uri.rsplit('/').take(2).collect();
    if components.len() != 2
        || components[1] != reference.sha256
        || components[0].parse::<u64>()? != reference.bytes
    {
        return Err("artifact CAS hash/size mismatch".into());
    }
    Ok(reference)
}

fn verify_binary(path: &Path, name: &str) -> Result<()> {
    let raw = fs::read(path)?;
    if raw.get(..6) != Some(&b"\x7fELF\x02\x01"[..]) || raw.get(18..20) != Some(&[62, 0][..]) {
        return Err("production artifact must be ELF amd64".into());
    }
    let marker = b"tl3.fx_canary_qualification_staging.v1";
    if raw.windows(marker.len()).any(|w| w == marker)
        != (name == "example_fx-stage-packet" || name == "example_test_fx-stage-packet_binary")
    {
        return Err("canary staging feature leaked or positive control missing".into());
    }
    Ok(())
}

// Preview only: the outer admitted BuildBuddy action owns terminal collection.
// Live qualification always uses build(), including its terminal/CAS checks.
pub fn admitted_preview(inputs: &Inputs) -> Result<Value> {
    let upload = std::env::var_os("BUILDBUDDY_ARTIFACTS_DIRECTORY")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute() && path.is_dir())
        .ok_or("preview build requires a BuildBuddy artifact directory")?;
    let root = inputs.out.join("production");
    fs::create_dir_all(&root)?;
    let log = root.join("remote.log");
    let output = fs::File::create(&log)?;
    let argv = build_argv(inputs, None)?;
    let status = Command::new(&argv[0])
        .args(&argv[1..])
        .current_dir(&inputs.repo)
        .env("BUILDBUDDY_ARTIFACTS_DIRECTORY", &root)
        .stdin(Stdio::null())
        .stdout(output.try_clone()?)
        .stderr(output)
        .status()?;
    if !status.success() {
        fs::copy(&log, upload.join("preview-build.log"))?;
        return Err("admitted preview build failed; collect outer invocation".into());
    }
    let record: Value = read_json(&root.join("runtime-build.json"))?;
    if record["base_head"] != inputs.head
        || record["tree"] != git(&inputs.repo, &["rev-parse", "HEAD^{tree}"])?
    {
        return Err("admitted preview source tree differs".into());
    }
    let mut artifacts = vec![];
    for artifact in record["artifacts"].as_array().ok_or("preview inventory")? {
        let name = artifact["name"].as_str().ok_or("preview artifact name")?;
        let path = root.join(name);
        verify_binary(&path, name)?;
        fs::copy(&path, upload.join(name))?;
        artifacts.push(identity(&path)?);
    }
    fs::copy(&log, upload.join("preview-build.log"))?;
    fs::copy(
        root.join("runtime-build.json"),
        upload.join("runtime-build.json"),
    )?;
    Ok(
        json!({"base_head":inputs.head,"overlay_tree":record["tree"],"command_exit":0,
        "invocation_id":null,"own_verdict":null,"collection":"outer_buildbuddy_action_required",
        "log":identity(&log)?,"artifacts":artifacts}),
    )
}

pub fn build(
    inputs: &Inputs,
    name: &str,
    overlay: Option<&BTreeMap<String, Vec<u8>>>,
) -> Result<Build> {
    let root = inputs.out.join(name);
    if !root.exists() {
        fs::create_dir(&root)?;
    }
    let argv = build_argv(inputs, overlay)?;
    let commit = &argv[3];
    let tree = expected_tree(inputs, &root, overlay)?;
    let result = root.join("build.json");
    if result.exists() {
        let build: Build = read_json(&result)?;
        verify_build_source(&build, &inputs.head, commit, &tree)?;
        verify_client()?;
        if build.client_sha256 != CLIENT_SHA256 {
            return Err("retained BuildBuddy client hash differs".into());
        }
        evidence::verify_references(&serde_json::to_value(&build)?)?;
        return Ok(build);
    }
    verify_client()?;
    let raw_argv = serde_json::to_vec(&argv)?;
    let argv_path = root.join("remote-build-argv.json");
    if argv_path.exists() {
        if durable::read(&argv_path)? != raw_argv {
            return Err("retained build input changed".into());
        }
    } else {
        durable::atomic(&argv_path, &raw_argv)?;
    }
    let submitted = root.join("submit.log");
    let raw = if submitted.exists() {
        fs::read_to_string(&submitted)?
    } else {
        if Path::new(HOLD).try_exists()? {
            return Err("BB-HOLD prevents submission".into());
        }
        // Persist intent before calling the external dispatcher. An empty/partial
        // log after interruption is uncertain, never permission to dispatch twice.
        write(
            &root.join("intent.json"),
            &json!({"head":inputs.head,"argv_sha256":durable::digest(&raw_argv),"client_sha256":CLIENT_SHA256}),
        )?;
        fs::File::create(&submitted)?.sync_all()?;
        let mut args = vec![
            "step".into(),
            "--repo".into(),
            "https://github.com/RobTand/trustladderiii".into(),
            "--commit".into(),
            commit.clone(),
            "--branch".into(),
            if overlay.is_some() {
                candidate(inputs, overlay.ok_or("overlay missing")?)?.branch
            } else {
                "fx-x3-qualify".into()
            },
            "--timeout".into(),
            "44".into(),
            "--max-wait".into(),
            "50".into(),
            "--log-file".into(),
            root.join("remote.log").to_string_lossy().into_owned(),
            "--".into(),
        ];
        args.extend(argv.clone());
        client(&args, &submitted)?
    };
    let id = invocation(&raw)?;
    let mut own = verdict(&raw);
    while own
        .as_ref()
        .is_none_or(|v| v["invocationStatus"] != "COMPLETE_INVOCATION_STATUS")
    {
        let reply = client(
            &[
                "wait".into(),
                id.clone(),
                "--max-wait".into(),
                "50".into(),
                "--log-file".into(),
                root.join("remote.log").to_string_lossy().into_owned(),
            ],
            &root.join("wait.log"),
        )?;
        own = verdict(&reply);
        if own.is_none() && !reply.contains("still running: ") {
            return Err("BuildBuddy terminal verdict unavailable".into());
        }
    }
    let mut own = own.ok_or("BuildBuddy own verdict absent")?;
    own["source"] = invocation_source(&id)?;
    if own["source"]["commitSha"] != *commit
        || own["source"]["invocationId"] != id
        || own["source"]["repoUrl"] != "https://github.com/RobTand/trustladderiii"
    {
        return Err("BuildBuddy invocation recorded another commit".into());
    }
    if own["success"] != true
        || !["OK", "SUCCESS"].contains(&own["bazelExitCode"].as_str().unwrap_or(""))
    {
        return Err("BuildBuddy terminal failure; see remote log".into());
    }
    let wrapper = fetch(&id, "bbrun-wrapper-result.v1.json", &root)?;
    let value: Value = read_json(&wrapper.path)?;
    for phase in ["command", "cleanup", "scanner", "artifact_handoff"] {
        if value[phase]["status"] != "pass" {
            return Err("BuildBuddy wrapper phase did not pass".into());
        }
    }
    let inventory = fetch(&id, "runtime-build.json", &root)?;
    let record: Value = read_json(&inventory.path)?;
    if record["base_head"] != *commit {
        return Err("remote build source mismatch".into());
    }
    if record["tree"] != tree {
        return Err("remote H plus generated overlay tree mismatch".into());
    }
    let mut artifacts = vec![serde_json::to_value(inventory)?];
    for artifact in record["artifacts"]
        .as_array()
        .ok_or("build artifact inventory")?
    {
        let name = artifact["name"].as_str().ok_or("build artifact name")?;
        if ![
            "tl3-run-backend",
            "tl3",
            "tl3d",
            "tl3-worker-supervisor",
            "example_fx-stage-packet",
            "example_generate-fx-qualification",
            "example_tl3-deploy",
            "lib_test_tl3_controller_binary",
            "example_test_fx-stage-packet_binary",
        ]
        .contains(&name)
        {
            return Err("unexpected production artifact".into());
        }
        let reference = fetch(&id, name, &root)?;
        if reference.sha256 != artifact["sha256"] || reference.bytes != artifact["bytes"] {
            return Err("remote producer artifact mismatch".into());
        }
        verify_binary(&reference.path, name)?;
        fs::set_permissions(&reference.path, fs::Permissions::from_mode(0o500))?;
        artifacts.push(identity(&reference.path)?);
    }
    let build = Build {
        invocation_id: id,
        client_sha256: CLIENT_SHA256.into(),
        base_head: inputs.head.clone(),
        build_commit: commit.clone(),
        overlay_tree: record["tree"].as_str().ok_or("build tree")?.into(),
        command_exit: 0,
        own_verdict: own,
        log: blob(&root.join("remote.log"))?,
        wrapper_result: wrapper,
        artifacts,
    };
    verify_build_source(&build, &inputs.head, commit, &tree)?;
    durable::atomic(&result, &serde_json::to_vec_pretty(&build)?)?;
    Ok(build)
}

const REMOTE_BUILD: &str = r#"import json, os, pathlib, subprocess, sys
head, qualified = sys.argv[1:3]
def run(argv):
    subprocess.run(argv, check=True)
assert subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip() == head
assert not subprocess.check_output(['git', 'status', '--porcelain', '--untracked-files=all'], text=True).strip()
targets = ['//crates/controller:' + name for name in ['tl3-run-backend', 'tl3', 'tl3d', 'tl3-worker-supervisor', 'example_fx-stage-packet', 'example_generate-fx-qualification', 'example_tl3-deploy']]
run(['bazel', 'build', '--config=remote', '--remote_download_outputs=toplevel', '--jobs=8', *targets])
run(['bazel-bin/crates/controller/example_fx-stage-packet', 'inventory-build', os.environ['BUILDBUDDY_ARTIFACTS_DIRECTORY']])
if qualified == 'true':
    run(['bazel', 'build', '--config=remote', '--remote_download_outputs=toplevel', '--jobs=8', '//crates/controller:lib_test_tl3_controller_binary', '//crates/controller:example_test_fx-stage-packet_binary'])
    run(['bazel-bin/crates/controller/example_fx-stage-packet', 'inventory-build', os.environ['BUILDBUDDY_ARTIFACTS_DIRECTORY']])
    run(['bazel-bin/crates/controller/example_generate-fx-qualification', '--repo', str(pathlib.Path.cwd() / 'crates/controller'), '--check'])
    run(['bazel', 'test', '--config=remote', '--jobs=8', '--test_output=errors', '//crates/controller:test_test_activation_qualification_absent'])
assert subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip() == head
assert not subprocess.check_output(['git', 'status', '--porcelain', '--untracked-files=all'], text=True).strip()
"#;

fn fixed_argv(commit: &str, qualified: bool) -> Vec<String> {
    vec![
        "/usr/bin/python3".into(),
        "-c".into(),
        REMOTE_BUILD.into(),
        commit.into(),
        qualified.to_string(),
    ]
}

// Both requests have the same fixed script and a 40-byte commit. Check the
// larger boolean spelling before admission; future evidence never enters argv.
pub fn preflight_transport(inputs: &Inputs) -> Result<()> {
    check_transport_size(&fixed_argv(&inputs.head, false))?;
    git(
        &inputs.repo,
        &[
            "push",
            "--dry-run",
            "--no-force",
            "origin",
            &format!(
                "{}:refs/heads/fx-x3-overlay/preflight-{}",
                inputs.head, inputs.head
            ),
        ],
    )
    .map_err(|_| "staging push preflight failed: origin must authenticate and permit dry-run pushes to fx-x3-overlay/*; no canaries admitted")?;
    Ok(())
}

fn build_argv(inputs: &Inputs, overlay: Option<&BTreeMap<String, Vec<u8>>>) -> Result<Vec<String>> {
    let commit = if let Some(overlay) = overlay {
        stage_overlay(inputs, &inputs.out.join("qualified-production"), overlay)?.commit
    } else {
        inputs.head.clone()
    };
    let argv = fixed_argv(&commit, overlay.is_some());
    check_transport_size(&argv)?;
    Ok(argv)
}

fn check_transport_size(argv: &[String]) -> Result<()> {
    // Pinned client: 4 * ceil((JSON bytes + 8192) / 3) <= 122880.
    // Its empty-command secured request is 60474 bytes. Reserve 64 KiB
    // for that wrapper, then leave another 4 KiB below the JSON ceiling.
    // The only variable wrapper fragment is shell-quoted Python repr(argv).
    let mut child = Command::new("/usr/bin/python3")
        .args(["-I", "-c", "import json, shlex, sys; argv=json.load(sys.stdin); size=lambda v: len(json.dumps(shlex.quote(repr(v))).encode()); print(65536 + size(argv) - size([]))"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    child
        .stdin
        .take()
        .ok_or("size check input absent")?
        .write_all(&serde_json::to_vec(argv)?)?;
    let output = child.wait_with_output()?;
    if !output.status.success() {
        return Err("BuildBuddy request size check failed".into());
    }
    let json_bytes: usize = std::str::from_utf8(&output.stdout)?.trim().parse()?;
    const JSON_LIMIT: usize = (122880 / 4) * 3 - 8192;
    if json_bytes > JSON_LIMIT - 4096 {
        return Err(format!(
            "BuildBuddy total JSON request bound {json_bytes} exceeds {} byte budget",
            JSON_LIMIT - 4096
        )
        .into());
    }
    Ok(())
}

fn source_mode(overlay: &BTreeMap<String, Vec<u8>>, path: &str) -> Result<u32> {
    let index: d8::current::Index =
        serde_json::from_slice(overlay.get(policy::INDEX).ok_or("overlay index missing")?)?;
    Ok(
        if path
            .strip_prefix(policy::INPUTS)
            .and_then(|name| index.fixtures.get(name))
            .is_some_and(|i| i.mode & 0o111 != 0)
        {
            0o100755
        } else {
            0o100644
        },
    )
}

fn obsolete(inputs: &Inputs, overlay: &BTreeMap<String, Vec<u8>>) -> Result<Vec<String>> {
    Ok(git(
        &inputs.repo,
        &[
            "ls-tree",
            "-r",
            "--name-only",
            &inputs.head,
            "--",
            policy::INPUTS,
        ],
    )?
    .lines()
    .filter(|path| !overlay.contains_key(*path))
    .map(String::from)
    .collect())
}

fn overlay_tree(
    inputs: &Inputs,
    root: &Path,
    overlay: Option<&BTreeMap<String, Vec<u8>>>,
) -> Result<String> {
    let Some(overlay) = overlay else {
        return git(
            &inputs.repo,
            &["rev-parse", &format!("{}^{{tree}}", inputs.head)],
        );
    };
    let index = root.join("overlay.index");
    let run = |args: &[&str], bytes: Option<&[u8]>| -> Result<String> {
        let mut command = Command::new("/usr/bin/git");
        command
            .current_dir(&inputs.repo)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("GIT_INDEX_FILE", &index)
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .stdin(if bytes.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            });
        let mut child = command.spawn()?;
        if let Some(bytes) = bytes {
            child
                .stdin
                .take()
                .ok_or("Git object input")?
                .write_all(bytes)?;
        }
        let output = child.wait_with_output()?;
        if !output.status.success() {
            return Err("generated overlay Git tree construction failed".into());
        }
        Ok(String::from_utf8(output.stdout)?.trim().into())
    };
    run(&["read-tree", &inputs.head], None)?;
    for path in obsolete(inputs, overlay)? {
        run(&["update-index", "--force-remove", "--", &path], None)?;
    }
    for (path, bytes) in overlay {
        let oid = run(&["hash-object", "-w", "--stdin"], Some(bytes))?;
        run(
            &[
                "update-index",
                "--add",
                "--cacheinfo",
                &format!("{:o}", source_mode(overlay, path)?),
                &oid,
                path,
            ],
            None,
        )?;
    }
    run(&["write-tree"], None)
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OverlayCommit {
    pub commit: String,
    pub tree: String,
    pub base_head: String,
    pub branch: String,
    pub overlay_sha256: String,
}

fn overlay_digest(overlay: &BTreeMap<String, Vec<u8>>) -> Result<String> {
    policy::publication_order(&overlay.keys().cloned().collect::<Vec<_>>())?;
    Ok(durable::digest(&serde_json::to_vec(overlay)?))
}

fn verify_overlay(
    inputs: &Inputs,
    root: &Path,
    overlay: &BTreeMap<String, Vec<u8>>,
    staged: &OverlayCommit,
) -> Result<()> {
    let digest = overlay_digest(overlay)?;
    if !api::hex(&staged.commit, 40)
        || staged.base_head != inputs.head
        || staged.overlay_sha256 != digest
        || staged.branch != format!("fx-x3-overlay/{}", &digest[..16])
        || git(
            &inputs.repo,
            &["rev-list", "--parents", "-n1", &staged.commit],
        )? != format!("{} {}", staged.commit, inputs.head)
        || git(
            &inputs.repo,
            &["rev-parse", &format!("{}^{{tree}}", staged.commit)],
        )? != staged.tree
    {
        return Err("overlay commit identity or exact parent H differs".into());
    }
    let changed = git(
        &inputs.repo,
        &[
            "diff",
            "--no-renames",
            "--name-only",
            &inputs.head,
            &staged.commit,
            "--",
        ],
    )?;
    if changed.lines().any(|path| !policy::derived(path)) {
        return Err("overlay commit contains non-derived paths".into());
    }
    if staged.tree != overlay_tree(inputs, root, Some(overlay))? {
        return Err("overlay commit blobs or modes differ from private derivation".into());
    }
    // Compare each blob with its privately generated bytes, including paths
    // unchanged from H. The tree equality above also checks removals and modes.
    for (path, bytes) in overlay {
        let output = Command::new("/usr/bin/git")
            .current_dir(&inputs.repo)
            .args(["cat-file", "blob", &format!("{}:{path}", staged.commit)])
            .stdin(Stdio::null())
            .output()?;
        if !output.status.success() || durable::digest(&output.stdout) != durable::digest(bytes) {
            return Err("overlay blob hash differs from private bytes".into());
        }
    }
    Ok(())
}

fn expected_tree(
    inputs: &Inputs,
    root: &Path,
    overlay: Option<&BTreeMap<String, Vec<u8>>>,
) -> Result<String> {
    let tree = overlay_tree(inputs, root, overlay)?;
    if let Some(overlay) = overlay {
        let record = root.join("overlay-commit.json");
        if record.exists() {
            let staged: OverlayCommit = read_json(&record)?;
            verify_overlay(inputs, root, overlay, &staged)?;
        }
    }
    Ok(tree)
}

fn stage_overlay(
    inputs: &Inputs,
    root: &Path,
    overlay: &BTreeMap<String, Vec<u8>>,
) -> Result<OverlayCommit> {
    fs::create_dir_all(root)?;
    let path = root.join("overlay-commit.json");
    let digest = overlay_digest(overlay)?;
    let branch = format!("fx-x3-overlay/{}", &digest[..16]);
    let reference = format!("refs/heads/{branch}");
    let tree = expected_tree(inputs, root, Some(overlay))?;
    let remote = git(&inputs.repo, &["ls-remote", "--refs", "origin", &reference])?;
    let existing = remote.split_whitespace().next();
    let staged = if path.exists() {
        serde_json::from_slice::<OverlayCommit>(&durable::read(&path)?)?
    } else {
        let commit = if let Some(commit) = existing {
            // Do not update an occupied branch, even when a normal push could
            // fast-forward it. Fetch its object and verify all content first.
            git(&inputs.repo, &["fetch", "--no-tags", "origin", &reference])?;
            commit.into()
        } else {
            git(
                &inputs.repo,
                &[
                    "commit-tree",
                    &tree,
                    "-p",
                    &inputs.head,
                    "-m",
                    &format!("chore(controller): stage qualification overlay {digest}"),
                ],
            )?
        };
        let tree = git(&inputs.repo, &["rev-parse", &format!("{commit}^{{tree}}")])?;
        OverlayCommit {
            commit,
            tree,
            base_head: inputs.head.clone(),
            branch,
            overlay_sha256: digest,
        }
    };
    verify_overlay(inputs, root, overlay, &staged)?;
    if existing.is_some_and(|commit| commit != staged.commit) {
        return Err("overlay branch already exists at another commit".into());
    }
    if !path.exists() {
        durable::atomic(&path, &serde_json::to_vec_pretty(&staged)?)?;
    }
    if existing.is_none() {
        git(
            &inputs.repo,
            &[
                "push",
                "--no-force",
                "origin",
                &format!("{}:{reference}", staged.commit),
            ],
        )?;
    }
    let observed = git(&inputs.repo, &["ls-remote", "--refs", "origin", &reference])?;
    if observed.split_whitespace().next() != Some(staged.commit.as_str()) {
        return Err("staged overlay remote readback differs".into());
    }
    Ok(staged)
}

pub fn candidate(inputs: &Inputs, overlay: &BTreeMap<String, Vec<u8>>) -> Result<OverlayCommit> {
    let root = inputs.out.join("qualified-production");
    let staged: OverlayCommit = read_json(&root.join("overlay-commit.json"))?;
    verify_overlay(inputs, &root, overlay, &staged)?;
    Ok(staged)
}

fn verify_build_source(build: &Build, head: &str, commit: &str, tree: &str) -> Result<()> {
    if build.base_head != head {
        return Err("BuildBuddy base head differs from H".into());
    }
    if build.build_commit != commit
        || build.overlay_tree != tree
        || build.own_verdict["source"]["commitSha"] != commit
        || build.own_verdict["source"]["repoUrl"] != "https://github.com/RobTand/trustladderiii"
        || build.own_verdict["source"]["invocationId"] != build.invocation_id
    {
        return Err("BuildBuddy invocation recorded another source commit or tree".into());
    }
    Ok(())
}

fn invocation_source(id: &str) -> Result<Value> {
    verify_client()?;
    let output = Command::new("/usr/bin/python3")
        .args([
            "-I",
            "-c",
            r#"
import hashlib, importlib.util, json, pathlib, sys
path, expected, invocation = sys.argv[1:]
assert hashlib.sha256(pathlib.Path(path).read_bytes()).hexdigest() == expected
spec = importlib.util.spec_from_file_location('bb', path)
bb = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bb)
client = bb.BuildBuddy((pathlib.Path.home()/'.config/buildbuddy/api_key').read_text().strip())
record = bb.status(client, invocation)
source = {field: record.get(field) for field in ['commitSha', 'repoUrl']}
source['invocationId'] = record.get('id', {}).get('invocationId')
print(bb.clean(client.key, json.dumps(source).encode()).decode())
"#,
            client_path().to_str().ok_or("UTF-8 client path required")?,
            CLIENT_SHA256,
            id,
        ])
        .stdin(Stdio::null())
        .output()?;
    if !output.status.success() {
        return Err("BuildBuddy invocation source lookup failed".into());
    }
    Ok(serde_json::from_slice(&output.stdout)?)
}

pub fn inventory_build(repo: &Path, out: &Path) -> Result<()> {
    let mut artifacts = Vec::new();
    for name in [
        "tl3-run-backend",
        "tl3",
        "tl3d",
        "tl3-worker-supervisor",
        "example_fx-stage-packet",
        "example_generate-fx-qualification",
        "example_tl3-deploy",
        "lib_test_tl3_controller_binary",
        "example_test_fx-stage-packet_binary",
    ] {
        let source = repo.join("bazel-bin/crates/controller").join(name);
        if name.ends_with("_binary") && !source.is_file() {
            continue;
        }
        let target = out.join(name);
        fs::copy(source, &target)?;
        let mut value = identity(&target)?;
        value["name"] = name.into();
        artifacts.push(value);
    }
    write(
        &out.join("runtime-build.json"),
        &json!({"base_head":git(&repo,&["rev-parse","HEAD"])?,"tree":git(&repo,&["write-tree"])?,"artifacts":artifacts}),
    )
}

#[cfg(test)]
mod staged_transport_tests {
    use super::*;

    fn fixture(root: &Path) -> (Inputs, BTreeMap<String, Vec<u8>>) {
        let repo = root.join("repo");
        let remote = root.join("remote.git");
        fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "--initial-branch=main"]).unwrap();
        git(&repo, &["init", "--bare", remote.to_str().unwrap()]).unwrap();
        fs::write(repo.join("source"), b"H\n").unwrap();
        git(&repo, &["add", "source"]).unwrap();
        git(&repo, &["commit", "-m", "source H"]).unwrap();
        git(
            &repo,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        )
        .unwrap();
        let mut inputs = super::super::tests::inputs(root);
        inputs.repo = repo;
        inputs.head = git(&inputs.repo, &["rev-parse", "HEAD"]).unwrap();
        let index = d8::current::Index {
            schema: "tl3.fx_qualification_inputs.v1".into(),
            source: inputs.head.clone(),
            inputs: BTreeMap::new(),
            runs: vec![],
            fixtures: BTreeMap::from([(
                "fixtures/helper $(literal).py".into(),
                d8::current::Input {
                    sha256: durable::digest(b"\0\xff'"),
                    bytes: 3,
                    mode: 0o755,
                },
            )]),
        };
        let overlay = BTreeMap::from([
            (policy::INDEX.into(), serde_json::to_vec(&index).unwrap()),
            (policy::PIN.into(), b"private pin".to_vec()),
            (
                format!("{}fixtures/helper $(literal).py", policy::INPUTS),
                b"\0\xff'".to_vec(),
            ),
            (
                format!("{}evidence.bin", policy::INPUTS),
                vec![37; 240 * 1024],
            ),
        ]);
        fs::create_dir_all(root.join("qualified-production")).unwrap();
        (inputs, overlay)
    }

    #[test]
    fn inventory_build_replaces_read_only_artifacts_and_refreshes_inventory() {
        let root = tempfile::tempdir().unwrap();
        let (inputs, _) = fixture(root.path());
        let source = inputs.repo.join("bazel-bin/crates/controller");
        let out = root.path().join("inventory");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir(&out).unwrap();
        let names = [
            "tl3-run-backend",
            "tl3",
            "tl3d",
            "tl3-worker-supervisor",
            "example_fx-stage-packet",
            "example_generate-fx-qualification",
            "example_tl3-deploy",
            "lib_test_tl3_controller_binary",
            "example_test_fx-stage-packet_binary",
        ];
        for name in &names[..7] {
            let path = source.join(name);
            fs::write(&path, b"first build").unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o555)).unwrap();
        }
        inventory_build(&inputs.repo, &out).unwrap();
        let first: Value = read_json(&out.join("runtime-build.json")).unwrap();
        assert_eq!(first["artifacts"].as_array().unwrap().len(), 7);
        for name in names {
            let path = source.join(name);
            if path.exists() {
                assert_eq!(
                    fs::metadata(out.join(name)).unwrap().permissions().mode() & 0o777,
                    0o555
                );
                fs::remove_file(&path).unwrap();
            }
            fs::write(&path, b"second build").unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o555)).unwrap();
        }
        inventory_build(&inputs.repo, &out)
            .expect("second inventory must replace read-only artifacts");
        let second: Value = read_json(&out.join("runtime-build.json")).unwrap();
        let artifacts = second["artifacts"].as_array().unwrap();
        assert_eq!(artifacts.len(), names.len());
        assert_eq!(second["base_head"], inputs.head);
        assert_eq!(second["tree"], git(&inputs.repo, &["write-tree"]).unwrap());
        for (artifact, name) in artifacts.iter().zip(names) {
            assert_eq!(artifact["name"], name);
            assert_eq!(artifact["sha256"], durable::digest(b"second build"));
            assert_eq!(artifact["bytes"], b"second build".len());
            assert_eq!(artifact["mode"], 0o555);
            assert_eq!(fs::read(out.join(name)).unwrap(), b"second build");
        }
    }

    #[test]
    fn staging_push_preflight_creates_no_remote_ref() {
        let root = tempfile::tempdir().unwrap();
        let (inputs, _) = fixture(root.path());
        let before = git(&inputs.repo, &["ls-remote", "--refs", "origin"]).unwrap();
        preflight_transport(&inputs).unwrap();
        assert_eq!(
            git(&inputs.repo, &["ls-remote", "--refs", "origin"]).unwrap(),
            before
        );
        assert_eq!(
            git(&inputs.repo, &["rev-parse", "HEAD"]).unwrap(),
            inputs.head
        );
        assert_eq!(git(&inputs.repo, &["status", "--porcelain"]).unwrap(), "");
    }

    #[test]
    fn staging_push_preflight_refuses_unavailable_receive_pack() {
        let root = tempfile::tempdir().unwrap();
        let (inputs, _) = fixture(root.path());
        let refuse = root.path().join("refuse-push");
        fs::write(&refuse, b"#!/bin/sh\nexit 1\n").unwrap();
        fs::set_permissions(&refuse, fs::Permissions::from_mode(0o700)).unwrap();
        git(
            &inputs.repo,
            &[
                "config",
                "remote.origin.receivepack",
                refuse.to_str().unwrap(),
            ],
        )
        .unwrap();
        assert_eq!(
            preflight_transport(&inputs).unwrap_err().to_string(),
            "staging push preflight failed: origin must authenticate and permit dry-run pushes to fx-x3-overlay/*; no canaries admitted"
        );
        git(
            &inputs.repo,
            &["config", "--unset", "remote.origin.receivepack"],
        )
        .unwrap();
        assert_eq!(
            git(&inputs.repo, &["ls-remote", "--refs", "origin"]).unwrap(),
            ""
        );
    }

    #[test]
    fn staged_overlay_refuses_occupied_child_with_different_tree() {
        let root = tempfile::tempdir().unwrap();
        let (inputs, overlay) = fixture(root.path());
        let tree = git(&inputs.repo, &["rev-parse", "HEAD^{tree}"]).unwrap();
        let wrong = git(
            &inputs.repo,
            &[
                "commit-tree",
                &tree,
                "-p",
                &inputs.head,
                "-m",
                "wrong overlay",
            ],
        )
        .unwrap();
        assert_ne!(wrong, inputs.head);
        let digest = overlay_digest(&overlay).unwrap();
        let branch = format!("refs/heads/fx-x3-overlay/{}", &digest[..16]);
        git(
            &inputs.repo,
            &["push", "origin", &format!("{wrong}:{branch}")],
        )
        .unwrap();
        assert_eq!(
            build_argv(&inputs, Some(&overlay)).unwrap_err().to_string(),
            "overlay commit blobs or modes differ from private derivation"
        );
        assert_eq!(
            git(&inputs.repo, &["ls-remote", "origin", &branch])
                .unwrap()
                .split_whitespace()
                .next()
                .unwrap(),
            wrong
        );
        assert!(
            !inputs
                .out
                .join("qualified-production/overlay-commit.json")
                .exists()
        );
    }

    fn retained_fixture(root: &Path) -> (Inputs, Build) {
        let (inputs, _) = fixture(root);
        let work = inputs.out.join("production");
        fs::create_dir(&work).unwrap();
        let reference = durable::blob(&work.join("reference.json"), b"{}").unwrap();
        let build = Build {
            invocation_id: "1".repeat(36),
            client_sha256: CLIENT_SHA256.into(),
            base_head: inputs.head.clone(),
            build_commit: inputs.head.clone(),
            overlay_tree: git(&inputs.repo, &["rev-parse", "HEAD^{tree}"]).unwrap(),
            command_exit: 0,
            own_verdict: json!({"source":{"commitSha":inputs.head,"repoUrl":"https://github.com/RobTand/trustladderiii","invocationId":"1".repeat(36)}}),
            log: reference.clone(),
            wrapper_result: reference,
            artifacts: vec![],
        };
        (inputs, build)
    }

    fn retain(inputs: &Inputs, value: &Build) {
        durable::atomic(
            &inputs.out.join("production/build.json"),
            &serde_json::to_vec(value).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn build_and_overlay_artifacts_round_trip_through_real_readers() {
        let root = tempfile::tempdir().unwrap();
        let _client = TestClient::install(root.path());
        let (inputs, expected) = retained_fixture(root.path());
        retain(&inputs, &expected);
        let observed = build(&inputs, "production", None).unwrap();
        assert_eq!(
            serde_json::to_value(observed).unwrap(),
            serde_json::to_value(expected).unwrap()
        );
        let overlay_root = tempfile::tempdir().unwrap();
        let (inputs, overlay) = fixture(overlay_root.path());
        let work = inputs.out.join("qualified-production");
        let staged = stage_overlay(&inputs, &work, &overlay).unwrap();
        let loaded = candidate(&inputs, &overlay).unwrap();
        assert_eq!(
            serde_json::to_value(&staged).unwrap(),
            serde_json::to_value(loaded).unwrap()
        );
        let publication = inputs.out.join("publication.json");
        durable::atomic(&publication, &serde_json::to_vec_pretty(&staged).unwrap()).unwrap();
        let published: OverlayCommit = read_json(&publication).unwrap();
        assert_eq!(published.commit, staged.commit);
        assert_eq!(published.tree, staged.tree);
    }

    include!("bb_client_fixture.rs");

    struct TestClient(PathBuf);

    impl TestClient {
        fn install(root: &Path) -> Self {
            let path = root.join("bbrun.py");
            fs::write(&path, CLIENT_FIXTURE.as_bytes()).unwrap();
            assert_eq!(durable::digest(&fs::read(&path).unwrap()), CLIENT_SHA256);
            Self(CLIENT.with(|client| client.replace(path)))
        }
    }

    impl Drop for TestClient {
        fn drop(&mut self) {
            CLIENT.with(|client| client.replace(self.0.clone()));
        }
    }

    #[test]
    fn retained_build_refuses_another_base_head() {
        let root = tempfile::tempdir().unwrap();
        let _client = TestClient::install(root.path());
        let (inputs, mut retained) = retained_fixture(root.path());
        retain(&inputs, &retained);
        super::build(&inputs, "production", None).unwrap();
        retained.base_head = "f".repeat(40);
        retain(&inputs, &retained);
        assert_eq!(
            super::build(&inputs, "production", None)
                .unwrap_err()
                .to_string(),
            "BuildBuddy base head differs from H"
        );
    }

    #[test]
    fn retained_build_refuses_wrong_client_sha256() {
        let root = tempfile::tempdir().unwrap();
        let _client = TestClient::install(root.path());
        let (inputs, mut retained) = retained_fixture(root.path());
        retain(&inputs, &retained);
        super::build(&inputs, "production", None).unwrap();
        retained.client_sha256 = "f".repeat(64);
        retain(&inputs, &retained);
        assert_eq!(
            super::build(&inputs, "production", None)
                .unwrap_err()
                .to_string(),
            "retained BuildBuddy client hash differs"
        );
    }

    #[test]
    fn retained_build_refuses_changed_client_file() {
        let root = tempfile::tempdir().unwrap();
        let _client = TestClient::install(root.path());
        let (inputs, retained) = retained_fixture(root.path());
        retain(&inputs, &retained);
        super::build(&inputs, "production", None).unwrap();
        fs::write(client_path(), b"changed client").unwrap();
        assert_eq!(
            super::build(&inputs, "production", None)
                .unwrap_err()
                .to_string(),
            "BuildBuddy client sha256 differs from pinned version"
        );
    }

    #[test]
    fn staged_overlay_larger_than_200kb_uses_a_git_commit() {
        let root = tempfile::tempdir().unwrap();
        let (inputs, mut overlay) = fixture(root.path());
        let mut state = 0x1234_5678_u32;
        for byte in overlay
            .get_mut(&format!("{}evidence.bin", policy::INPUTS))
            .unwrap()
        {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            *byte = state as u8;
        }
        assert!(overlay.values().map(Vec::len).sum::<usize>() > 200 * 1024);
        let argv =
            build_argv(&inputs, Some(&overlay)).expect("large overlays must travel through Git");
        assert_eq!(argv.len(), 5, "overlay bytes must not enter argv");
        assert_ne!(argv[3], inputs.head);
        assert_eq!(
            git(&inputs.repo, &["rev-parse", &format!("{}^", argv[3])]).unwrap(),
            inputs.head
        );
        assert_eq!(
            git(&inputs.repo, &["rev-parse", "HEAD"]).unwrap(),
            inputs.head
        );
        assert!(
            git(&inputs.repo, &["status", "--porcelain"])
                .unwrap()
                .is_empty()
        );
        for (path, bytes) in &overlay {
            let raw = Command::new("/usr/bin/git")
                .current_dir(&inputs.repo)
                .args(["cat-file", "blob", &format!("{}:{path}", argv[3])])
                .output()
                .unwrap();
            assert!(raw.status.success());
            assert_eq!(durable::digest(&raw.stdout), durable::digest(bytes));
        }
        assert!(
            git(&inputs.repo, &["ls-tree", "-r", &argv[3]])
                .unwrap()
                .lines()
                .any(|line| line.starts_with("100755 ") && line.ends_with("helper $(literal).py"))
        );
        let again = build_argv(&inputs, Some(&overlay)).unwrap();
        assert_eq!(again, argv, "resumption must reuse the exact commit");
    }

    #[test]
    fn staged_overlay_refuses_existing_branch_with_different_content() {
        let root = tempfile::tempdir().unwrap();
        let (inputs, overlay) = fixture(root.path());
        let digest = durable::digest(&serde_json::to_vec(&overlay).unwrap());
        let branch = format!("refs/heads/fx-x3-overlay/{}", &digest[..16]);
        git(
            &inputs.repo,
            &["push", "origin", &format!("{}:{branch}", inputs.head)],
        )
        .unwrap();
        assert_eq!(
            build_argv(&inputs, Some(&overlay)).unwrap_err().to_string(),
            "overlay commit identity or exact parent H differs"
        );
        assert_eq!(
            git(&inputs.repo, &["ls-remote", "origin", &branch])
                .unwrap()
                .split_whitespace()
                .next()
                .unwrap(),
            inputs.head
        );
    }

    #[test]
    fn staged_overlay_refuses_extra_paths() {
        refuses_changed_overlay(true);
    }

    #[test]
    fn staged_overlay_refuses_changed_blobs() {
        refuses_changed_overlay(false);
    }

    fn refuses_changed_overlay(extra_path: bool) {
        let root = tempfile::tempdir().unwrap();
        let (inputs, overlay) = fixture(root.path());
        let work = inputs.out.join("qualified-production");
        let tree = expected_tree(&inputs, &work, Some(&overlay)).unwrap();
        git(&inputs.repo, &["read-tree", &tree]).unwrap();
        let path = if extra_path {
            "unapproved"
        } else {
            policy::PIN
        };
        fs::create_dir_all(inputs.repo.join(path).parent().unwrap()).unwrap();
        fs::write(inputs.repo.join(path), b"different bytes").unwrap();
        git(&inputs.repo, &["add", "--", path]).unwrap();
        let wrong_tree = git(&inputs.repo, &["write-tree"]).unwrap();
        let commit = git(
            &inputs.repo,
            &[
                "commit-tree",
                &wrong_tree,
                "-p",
                &inputs.head,
                "-m",
                "invalid overlay",
            ],
        )
        .unwrap();
        let digest = durable::digest(&serde_json::to_vec(&overlay).unwrap());
        write(&work.join("overlay-commit.json"), &json!({"commit":commit,"tree":wrong_tree,"base_head":inputs.head,"branch":format!("fx-x3-overlay/{}", &digest[..16]),"overlay_sha256":digest})).unwrap();
        assert_eq!(
            expected_tree(&inputs, &work, Some(&overlay))
                .unwrap_err()
                .to_string(),
            if extra_path {
                "overlay commit contains non-derived paths"
            } else {
                "overlay commit blobs or modes differ from private derivation"
            }
        );
    }

    #[test]
    fn retained_build_refuses_another_recorded_commit() {
        let root = tempfile::tempdir().unwrap();
        let (inputs, _) = fixture(root.path());
        let work = inputs.out.join("production");
        fs::create_dir(&work).unwrap();
        let reference = durable::blob(&work.join("reference.json"), b"{}").unwrap();
        let build = Build {
            invocation_id: "1".repeat(36),
            client_sha256: CLIENT_SHA256.into(),
            base_head: inputs.head.clone(),
            build_commit: inputs.head.clone(),
            overlay_tree: git(&inputs.repo, &["rev-parse", "HEAD^{tree}"]).unwrap(),
            command_exit: 0,
            own_verdict: json!({"commitSha":"f".repeat(40),"success":true,"invocationStatus":"COMPLETE_INVOCATION_STATUS","bazelExitCode":"OK"}),
            log: reference.clone(),
            wrapper_result: reference,
            artifacts: vec![],
        };
        let tree = build.overlay_tree.clone();
        assert_eq!(
            verify_build_source(&build, &inputs.head, &inputs.head, &tree)
                .unwrap_err()
                .to_string(),
            "BuildBuddy invocation recorded another source commit or tree"
        );
        let mut accepted = build.clone();
        accepted.own_verdict["source"] = json!({"commitSha":inputs.head,"repoUrl":"https://github.com/RobTand/trustladderiii","invocationId":build.invocation_id});
        assert!(verify_build_source(&accepted, &inputs.head, &inputs.head, &tree).is_ok());
        accepted.own_verdict["source"]["commitSha"] = "f".repeat(40).into();
        assert_eq!(
            verify_build_source(&accepted, &inputs.head, &inputs.head, &tree)
                .unwrap_err()
                .to_string(),
            "BuildBuddy invocation recorded another source commit or tree"
        );
        write(&work.join("build.json"), &accepted).unwrap();
        assert_eq!(
            super::build(&inputs, "production", None)
                .expect_err("retained invocation commit must be checked")
                .to_string(),
            "BuildBuddy invocation recorded another source commit or tree"
        );
    }
}
