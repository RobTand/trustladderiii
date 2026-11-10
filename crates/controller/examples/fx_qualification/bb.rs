use super::*;
use std::{
    io::Write,
    process::{Command, Stdio},
};

const CLIENT: &str = "/home/rob/tmp/bazel-wave2/tools/bbrun.py";
const HOLD: &str = "/home/rob/tmp/tl3-build/rust-fv/BB-HOLD";

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Build {
    pub invocation_id: String,
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

fn client(args: &[String], out: &Path) -> Result<String> {
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
    let status = Command::new("/bin/bash")
        .args(["-c", &script(inputs, None)?])
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
        evidence::verify_references(&serde_json::to_value(&build)?)?;
        return Ok(build);
    }
    let script = script(inputs, overlay)?;
    let script_path = root.join("remote-build.sh");
    if script_path.exists() {
        if durable::read(&script_path)? != script.as_bytes() {
            return Err("retained build input changed".into());
        }
    } else {
        durable::atomic(&script_path, script.as_bytes())?;
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
            &json!({"head":inputs.head,"script_sha256":durable::digest(script.as_bytes())}),
        )?;
        fs::File::create(&submitted)?.sync_all()?;
        client(
            &[
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
                "/bin/bash".into(),
                "-c".into(),
                script,
            ],
            &submitted,
        )?
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

fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

fn script(inputs: &Inputs, overlay: Option<&BTreeMap<String, Vec<u8>>>) -> Result<String> {
    use base64::Engine;
    let mut script = format!(
        "set -euo pipefail\ntest \"$(git rev-parse HEAD)\" = {}\n",
        quote(&inputs.head)
    );
    if let Some(overlay) = overlay {
        policy::publication_order(&overlay.keys().cloned().collect::<Vec<_>>())?;
        for path in obsolete(inputs, overlay)? {
            script.push_str(&format!("rm -- {}\n", quote(&path)));
        }
        for (path, bytes) in overlay {
            script.push_str(&format!(
                "mkdir -p {}\nprintf %s {} | base64 -d > {}\n",
                quote(
                    Path::new(path)
                        .parent()
                        .ok_or("overlay parent")?
                        .to_str()
                        .ok_or("overlay UTF-8")?
                ),
                quote(&base64::engine::general_purpose::STANDARD.encode(bytes)),
                quote(path)
            ));
            script.push_str(&format!(
                "chmod {} {}\n",
                if source_mode(overlay, path)? == 0o100755 {
                    "755"
                } else {
                    "644"
                },
                quote(path)
            ));
        }
        script.push_str("git add -- crates/controller/qualification/fx-current crates/controller/qualification/current.json crates/controller/src/adapters/flux_checks/activation/qualification.json\n");
    }
    script.push_str("bazel build --config=remote --jobs=8 //crates/controller:tl3-run-backend //crates/controller:tl3 //crates/controller:tl3d //crates/controller:tl3-worker-supervisor //crates/controller:example_fx-stage-packet //crates/controller:example_generate-fx-qualification //crates/controller:example_tl3-deploy\nbazel-bin/crates/controller/example_fx-stage-packet inventory-build \"$BUILDBUDDY_ARTIFACTS_DIRECTORY\"\n");
    if overlay.is_some() {
        script.push_str("bazel build --config=remote --jobs=8 //crates/controller:lib_test_tl3_controller_binary //crates/controller:example_test_fx-stage-packet_binary\nbazel-bin/crates/controller/example_fx-stage-packet inventory-build \"$BUILDBUDDY_ARTIFACTS_DIRECTORY\"\n");
        script.push_str("bazel-bin/crates/controller/example_generate-fx-qualification --repo \"$PWD/crates/controller\" --check\nbazel test --config=remote --jobs=8 --test_output=errors //crates/controller:test_test_activation_qualification_absent\n");
    }
    Ok(script)
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
