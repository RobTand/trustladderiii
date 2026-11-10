use super::*;
use std::{
    io::Write,
    process::{Command, Stdio},
};

const CLIENT: &str = "/home/rob/tmp/bazel-wave2/tools/bbrun.py";
const CLIENT_SHA256: &str = "8016e41d97aac7e83f7b5b39c57aea3c030a35d555e35f2929f09ab6076d7aa0";
const HOLD: &str = "/home/rob/tmp/tl3-build/rust-fv/BB-HOLD";

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Build {
    pub invocation_id: String,
    pub client_sha256: String,
    pub base_head: String,
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
    if durable::digest(&durable::read(Path::new(CLIENT))?) != CLIENT_SHA256 {
        return Err("BuildBuddy client sha256 differs from pinned version".into());
    }
    Ok(())
}

fn client(args: &[String], out: &Path) -> Result<String> {
    verify_client()?;
    let output = Command::new("/usr/bin/python3")
        .arg("-I")
        .arg(CLIENT)
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
    let record: Value = serde_json::from_slice(&durable::read(&root.join("runtime-build.json"))?)?;
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
    let result = root.join("build.json");
    if result.exists() {
        let build: Build = serde_json::from_slice(&durable::read(&result)?)?;
        verify_client()?;
        if build.client_sha256 != CLIENT_SHA256 {
            return Err("retained BuildBuddy client hash differs".into());
        }
        evidence::verify_references(&serde_json::to_value(&build)?)?;
        return Ok(build);
    }
    verify_client()?;
    let argv = build_argv(inputs, overlay)?;
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
            inputs.head.clone(),
            "--branch".into(),
            "fx-x3-qualify".into(),
            "--timeout".into(),
            "60".into(),
            "--max-wait".into(),
            "50".into(),
            "--log-file".into(),
            root.join("remote.log").to_string_lossy().into_owned(),
            "--".into(),
        ];
        args.extend(argv);
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
    let own = own.ok_or("BuildBuddy own verdict absent")?;
    if own["success"] != true
        || !["OK", "SUCCESS"].contains(&own["bazelExitCode"].as_str().unwrap_or(""))
    {
        return Err("BuildBuddy terminal failure; see remote log".into());
    }
    let wrapper = fetch(&id, "bbrun-wrapper-result.v1.json", &root)?;
    let value: Value = serde_json::from_slice(&durable::read(&wrapper.path)?)?;
    for phase in ["command", "cleanup", "scanner", "artifact_handoff"] {
        if value[phase]["status"] != "pass" {
            return Err("BuildBuddy wrapper phase did not pass".into());
        }
    }
    let inventory = fetch(&id, "runtime-build.json", &root)?;
    let record: Value = serde_json::from_slice(&durable::read(&inventory.path)?)?;
    if record["base_head"] != inputs.head {
        return Err("remote build source mismatch".into());
    }
    if record["tree"] != expected_tree(inputs, &root, overlay)? {
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
        overlay_tree: record["tree"].as_str().ok_or("build tree")?.into(),
        command_exit: 0,
        own_verdict: own,
        log: blob(&root.join("remote.log"))?,
        wrapper_result: wrapper,
        artifacts,
    };
    durable::atomic(&result, &serde_json::to_vec_pretty(&build)?)?;
    Ok(build)
}

const REMOTE_BUILD: &str = r#"import base64, gzip, hashlib, json, os, pathlib, subprocess, sys
head, qualified = sys.argv[1:3]
payload = json.loads(gzip.decompress(base64.b64decode(''.join(sys.argv[3:]))))
def run(argv):
    subprocess.run(argv, check=True)
assert subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip() == head
for name in payload['obsolete']:
    pathlib.Path(name).unlink()
for entry in payload['files']:
    path = pathlib.Path(entry['path'])
    path.parent.mkdir(parents=True, exist_ok=True)
    data = pathlib.Path(entry['source']).read_bytes() if 'source' in entry else base64.b64decode(entry['data'])
    assert hashlib.sha256(data).hexdigest() == entry['sha256']
    path.write_bytes(data)
    path.chmod(entry['mode'])
if qualified == 'true':
    run(['git', 'add', '--', 'crates/controller/qualification/fx-current', 'crates/controller/qualification/current.json', 'crates/controller/src/adapters/flux_checks/activation/qualification.json'])
targets = ['//crates/controller:' + name for name in ['tl3-run-backend', 'tl3', 'tl3d', 'tl3-worker-supervisor', 'example_fx-stage-packet', 'example_generate-fx-qualification', 'example_tl3-deploy']]
run(['bazel', 'build', '--config=remote', '--remote_download_outputs=toplevel', '--jobs=8', *targets])
run(['bazel-bin/crates/controller/example_fx-stage-packet', 'inventory-build', os.environ['BUILDBUDDY_ARTIFACTS_DIRECTORY']])
if qualified == 'true':
    run(['bazel', 'build', '--config=remote', '--remote_download_outputs=toplevel', '--jobs=8', '//crates/controller:lib_test_tl3_controller_binary', '//crates/controller:example_test_fx-stage-packet_binary'])
    run(['bazel-bin/crates/controller/example_fx-stage-packet', 'inventory-build', os.environ['BUILDBUDDY_ARTIFACTS_DIRECTORY']])
    run(['bazel-bin/crates/controller/example_generate-fx-qualification', '--repo', str(pathlib.Path.cwd() / 'crates/controller'), '--check'])
    run(['bazel', 'test', '--config=remote', '--jobs=8', '--test_output=errors', '//crates/controller:test_test_activation_qualification_absent'])
"#;

fn gzip(bytes: Vec<u8>, decompress: bool) -> Result<Vec<u8>> {
    let mut child = Command::new("/usr/bin/gzip")
        .args(if decompress {
            &["-d", "-c"][..]
        } else {
            &["-n", "-c"][..]
        })
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let mut stdin = child.stdin.take().ok_or("gzip input absent")?;
    let writer = std::thread::spawn(move || stdin.write_all(&bytes));
    let output = child.wait_with_output()?;
    writer.join().map_err(|_| "gzip writer panicked")??;
    if !output.status.success() {
        return Err("overlay gzip failed".into());
    }
    Ok(output.stdout)
}

fn build_argv(inputs: &Inputs, overlay: Option<&BTreeMap<String, Vec<u8>>>) -> Result<Vec<String>> {
    use base64::Engine;
    let mut files = vec![];
    let mut removed = vec![];
    if let Some(overlay) = overlay {
        policy::publication_order(&overlay.keys().cloned().collect::<Vec<_>>())?;
        removed = obsolete(inputs, overlay)?;
        for (path, bytes) in overlay {
            let mut entry = json!({"path":path,"mode":source_mode(overlay,path)? & 0o777,"sha256":durable::digest(bytes)});
            let source = path
                .strip_prefix(&format!("{}fixtures/lib/tl3_harness/", policy::INPUTS))
                .filter(|name| name.ends_with(".py"))
                .map(|name| format!("harness/src/tl3_harness/{name}"))
                .filter(|source| fs::read(inputs.repo.join(source)).is_ok_and(|raw| raw == *bytes));
            if let Some(source) = source {
                // The exact H checkout already carries these bytes. The remote
                // hash check binds the copy without uploading the code twice.
                entry["source"] = source.into();
            } else {
                entry["data"] = base64::engine::general_purpose::STANDARD
                    .encode(bytes)
                    .into();
            }
            files.push(entry);
        }
    }
    let payload = base64::engine::general_purpose::STANDARD.encode(gzip(
        serde_json::to_vec(&json!({"files":files,"obsolete":removed}))?,
        false,
    )?);
    // Leave room for the pinned client's secured wrappers and JSON escaping.
    if payload.len() + REMOTE_BUILD.len() > 80 * 1024 {
        return Err("compressed BuildBuddy overlay exceeds 80 KiB action budget".into());
    }
    let mut argv = vec![
        "/usr/bin/python3".into(),
        "-c".into(),
        REMOTE_BUILD.into(),
        inputs.head.clone(),
        overlay.is_some().to_string(),
    ];
    for chunk in payload.as_bytes().chunks(16 * 1024) {
        argv.push(std::str::from_utf8(chunk)?.into());
    }
    Ok(argv)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn overlay_arguments_preserve_paths_bytes_and_modes_without_shell_interpolation() {
        use base64::Engine;
        let root = tempfile::tempdir().unwrap();
        let mut inputs = super::super::tests::inputs(root.path());
        inputs.head = git(&inputs.repo, &["rev-parse", "HEAD"]).unwrap();
        let index = d8::current::Index {
            schema: "tl3.fx_qualification_inputs.v1".into(),
            source: inputs.head.clone(),
            inputs: BTreeMap::new(),
            runs: vec![],
            fixtures: BTreeMap::from([(
                "fixtures/helper $(literal).py".into(),
                d8::current::Input {
                    sha256: "a".repeat(64),
                    bytes: 3,
                    mode: 0o755,
                },
            )]),
        };
        let name = format!("{}fixtures/helper $(literal).py", policy::INPUTS);
        let mut overlay = BTreeMap::from([
            (policy::INDEX.into(), serde_json::to_vec(&index).unwrap()),
            (policy::PIN.into(), b"pin".to_vec()),
            (name.clone(), b"\0\xff'".to_vec()),
        ]);
        for entry in fs::read_dir(inputs.repo.join("harness/src/tl3_harness")).unwrap() {
            let entry = entry.unwrap();
            if entry
                .path()
                .extension()
                .is_some_and(|extension| extension == "py")
            {
                overlay.insert(
                    format!(
                        "{}fixtures/lib/tl3_harness/{}",
                        policy::INPUTS,
                        entry.file_name().to_str().unwrap()
                    ),
                    fs::read(entry.path()).unwrap(),
                );
            }
        }
        assert!(overlay.values().map(Vec::len).sum::<usize>() > 128 * 1024);
        let argv = build_argv(&inputs, Some(&overlay)).unwrap();
        assert_eq!(&argv[..2], ["/usr/bin/python3", "-c"]);
        assert_eq!(argv[3], inputs.head);
        assert_eq!(argv[4], "true");
        assert!(argv.iter().all(|arg| arg.len() < 32 * 1024));
        let payload: Value = serde_json::from_slice(
            &gzip(
                base64::engine::general_purpose::STANDARD
                    .decode(argv[5..].concat())
                    .unwrap(),
                true,
            )
            .unwrap(),
        )
        .unwrap();
        let files = payload["files"].as_array().unwrap();
        assert_eq!(files.len(), overlay.len());
        for file in files {
            let path = file["path"].as_str().unwrap();
            let bytes = if let Some(source) = file["source"].as_str() {
                assert!(file.get("data").is_none());
                fs::read(inputs.repo.join(source)).unwrap()
            } else {
                base64::engine::general_purpose::STANDARD
                    .decode(file["data"].as_str().unwrap())
                    .unwrap()
            };
            assert_eq!(bytes, overlay[path]);
            assert_eq!(file["sha256"], durable::digest(&bytes));
            assert_eq!(file["mode"], if path == name { 0o755 } else { 0o644 });
        }
        assert_eq!(build_argv(&inputs, None).unwrap()[4], "false");
        // Incompressible evidence can fit the old 80 KiB payload guard while
        // exceeding the client's total JSON budget once wrappers are included.
        let mut state = 0x1234_5678_u32;
        let bytes: Vec<u8> = (0..48 * 1024).map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state as u8
        }).collect();
        overlay.insert(name, bytes);
        assert!(build_argv(&inputs, Some(&overlay)).is_err());

    }
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

fn expected_tree(
    inputs: &Inputs,
    root: &Path,
    overlay: Option<&BTreeMap<String, Vec<u8>>>,
) -> Result<String> {
    let Some(overlay) = overlay else {
        return git(&inputs.repo, &["rev-parse", "HEAD^{tree}"]);
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

pub fn inventory_build(out: &Path) -> Result<()> {
    let repo = std::env::current_dir()?;
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
