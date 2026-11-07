//! Rust owns worker transport publication, reservations and canonical backend requests.
use super::*;
use schemars::JsonSchema;
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    os::unix::fs::PermissionsExt,
};

#[derive(Deserialize, JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum Command {
    Allocate {
        profile: PathBuf,
        call: String,
    },
    Prepare {
        profile: PathBuf,
        call: String,
        served_tree: String,
        contract: String,
        generator: bool,
        image: String,
        cpus: u32,
        mem_gb: u32,
        parallel: u32,
        quiet_seconds: u32,
        walltime_seconds: u32,
        arguments: Vec<String>,
    },
    Resume {
        profile: PathBuf,
        key: String,
        served_tree: String,
        contract: String,
    },
    Activation,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Prepared {
    wait: WaitBinding,
    request: backend::Request,
    contract: String,
}

fn profile(path: &Path) -> Result<(Profile, backend::Options)> {
    let profile: Profile = serde_json::from_slice(&durable::read(path).map_err(backend_error)?)?;
    if profile.schema != "tl3.worker_execution.v1"
        || path != profile.attempt.join("execution.json")
        || !backend::absolute(&profile.waits_root)
        || !backend::absolute(&profile.staging_root)
    {
        return Err(Error::Invalid("invalid retained worker execution profile"));
    }
    let request = read(&profile.attempt.join("request.json"))?
        .ok_or(Error::Invalid("missing retained worker attempt"))?;
    if request["inputs"]["tests"]["execution"]["profile"] != json!(path)
        || request["worktree"]
            .as_str()
            .map(Path::new)
            .map(fs::canonicalize)
            .transpose()?
            .as_ref()
            != Some(&profile.checkout)
    {
        return Err(Error::Invalid(
            "execution profile belongs to another worker checkout",
        ));
    }
    let options = serde_json::from_slice(&durable::read(&profile.options).map_err(backend_error)?)?;
    Ok((profile, options))
}

fn root(profile: &Profile, call: &str) -> Result<PathBuf> {
    if !backend::hex(call, 64) {
        return Err(Error::Invalid(
            "worker call requires a fresh retained identity",
        ));
    }
    let parent = profile.staging_root.join("workers");
    fs::create_dir_all(&parent)?;
    if parent.canonicalize()?.starts_with(&profile.checkout) {
        return Err(Error::Invalid(
            "worker input must be outside served checkout",
        ));
    }
    for ancestor in parent.ancestors() {
        if ancestor.is_symlink() {
            return Err(Error::Invalid(
                "worker staging ancestors cannot be symlinks",
            ));
        }
    }
    Ok(parent.join(durable::digest(&serde_json::to_vec(&(
        &profile.attempt,
        call,
    ))?)))
}

pub(super) fn reservation(cpus: u32, mem_gb: u32, parallel: u32) -> Result<backend::Reservation> {
    let parallel = parallel.max(1);
    let cpus = cpus.max(parallel);
    let memory = u64::from(mem_gb)
        .checked_mul(u64::from(parallel))
        .and_then(|n| n.checked_mul(1 << 30))
        .ok_or(Error::Invalid("worker memory reservation overflow"))?;
    Ok(backend::Reservation {
        cpus: cpus
            .try_into()
            .map_err(|_| Error::Invalid("positive worker cpus required"))?,
        gpus: 0,
        memory_bytes: memory
            .try_into()
            .map_err(|_| Error::Invalid("positive worker memory required"))?,
        memory_guarantee: backend::MemoryGuarantee::DeclaredOnly,
        parallelism: parallel.try_into().expect("positive parallelism"),
        native_threads: (cpus / parallel)
            .max(1)
            .try_into()
            .expect("positive native threads"),
    })
}

fn git(root: &Path, args: &[&str]) -> Result<String> {
    let mut command = std::process::Command::new("git");
    command
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", "/nonexistent")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "TL3")
        .env("GIT_AUTHOR_EMAIL", "tl3@example.invalid")
        .env("GIT_COMMITTER_NAME", "TL3")
        .env("GIT_COMMITTER_EMAIL", "tl3@example.invalid")
        .args([
            "--no-replace-objects",
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.fsmonitor=false",
        ])
        .args(args)
        .current_dir(root);
    let output = command.output()?;
    if !output.status.success() {
        return Err(Error::Invalid("worker transport Git publication failed"));
    }
    String::from_utf8(output.stdout)
        .map(|s| s.trim().to_owned())
        .map_err(|_| Error::Invalid("worker transport Git output is not UTF-8"))
}

fn inventory(root: &Path, directory: &Path, files: &mut Vec<backend::InputFile>) -> Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        if directory == root && entry.file_name() == ".git" {
            continue;
        }
        let path = entry.path();
        let kind = entry.file_type()?;
        if kind.is_dir() {
            inventory(root, &path, files)?;
        } else if kind.is_file() {
            if files.len() >= 100_000 {
                return Err(Error::Invalid("worker input inventory exceeds bound"));
            }
            let mut input = File::open(&path)?;
            let mut digest = Sha256::new();
            let mut chunk = [0_u8; 65536];
            loop {
                let read = input.read(&mut chunk)?;
                if read == 0 {
                    break;
                }
                digest.update(&chunk[..read]);
            }
            files.push(backend::InputFile {
                path: path
                    .strip_prefix(root)
                    .map_err(|_| Error::Invalid("input escaped publication"))?
                    .to_owned(),
                sha256: format!("{:x}", digest.finalize()),
            });
            let mode = fs::metadata(&path)?.permissions().mode();
            fs::set_permissions(
                &path,
                fs::Permissions::from_mode(if mode & 0o111 != 0 { 0o500 } else { 0o400 }),
            )?;
            File::open(&path)?.sync_all()?;
        } else {
            return Err(Error::Invalid("worker input contains a nonregular file"));
        }
    }
    fs::set_permissions(directory, fs::Permissions::from_mode(0o500))?;
    File::open(directory)?.sync_all()?;
    Ok(())
}

fn response(prepared: &Prepared, profile: &Profile, options: &backend::Options) -> Result<Value> {
    let key = prepared.wait.key()?;
    Ok(
        json!({"key": key, "tree": prepared.wait.served_tree, "contract": prepared.contract,
        "cli": options.bootstrap.cli, "config": profile.options,
        "bounds": backend::output_bounds(&prepared.request.suite),
        "submit": backend::cli::Command::Submit { request: Box::new(prepared.request.clone()), intent: prepared.wait.intent.clone() },
        "attach": backend::cli::Command::Attach { owner: prepared.wait.owner.owner.clone(), correlation: prepared.wait.owner.correlation.clone() },
        "owner": prepared.wait.owner }),
    )
}

fn execute(command: Command) -> Result<Value> {
    match command {
        Command::Activation => {
            crate::adapters::flux_checks::staging::require_runtime_activation()?;
            Ok(json!({"allowed": true}))
        }
        Command::Allocate {
            profile: path,
            call,
        } => {
            let (profile, _) = profile(&path)?;
            let root = root(&profile, &call)?;
            fs::create_dir(&root)?;
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
            File::open(root.parent().expect("worker input parent"))?.sync_all()?;
            Ok(json!({"root": root}))
        }
        Command::Resume {
            profile: path,
            key,
            served_tree,
            contract,
        } => {
            let (profile, options) = profile(&path)?;
            let id = key
                .strip_prefix("worker:")
                .filter(|id| backend::hex(id, 64))
                .ok_or(Error::Invalid("invalid worker wait reference"))?;
            let prepared: Prepared = serde_json::from_slice(
                &durable::read(&profile.waits_root.join(format!("{id}.prepared.json")))
                    .map_err(backend_error)?,
            )?;
            prepared.wait.validate(&key, &profile.checkout, &options)?;
            if prepared.wait.profile != path
                || prepared.wait.served_tree != served_tree
                || prepared.contract != contract
            {
                return Err(Error::Invalid(
                    "worker reattachment contract changed; retain original inputs",
                ));
            }
            response(&prepared, &profile, &options)
        }
        Command::Prepare {
            profile: path,
            call,
            served_tree,
            contract,
            generator,
            image,
            cpus,
            mem_gb,
            parallel,
            quiet_seconds,
            walltime_seconds,
            arguments,
        } => {
            let (profile, options) = profile(&path)?;
            if !backend::hex(&served_tree, 40)
                || !backend::hex(&contract, 64)
                || arguments.len() > 256
            {
                return Err(Error::Invalid("invalid worker source or contract identity"));
            }
            let root = root(&profile, &call)?;
            if !root.is_dir() || root.is_symlink() {
                return Err(Error::Invalid("worker input was not allocated"));
            }
            if durable::digest(
                &durable::read(&root.join("worker-input.json")).map_err(backend_error)?,
            ) != contract
            {
                return Err(Error::Invalid(
                    "worker protocol input differs from its retained contract",
                ));
            }
            // PB intentionally leaves a dirty snapshot. Flux commits the transport independently.
            git(&root, &["add", "--force", "--all"])?;
            git(
                &root,
                &[
                    "commit",
                    "--quiet",
                    "--allow-empty",
                    "-m",
                    "Freeze worker transport input",
                ],
            )?;
            let head = git(&root, &["rev-parse", "HEAD"])?;
            let tree = git(&root, &["rev-parse", "HEAD^{tree}"])?;
            let reservation = reservation(cpus, mem_gb, parallel)?;
            let mut runner_args = vec![
                "--timeout".into(),
                walltime_seconds.to_string(),
                "--".into(),
                "/usr/bin/python3.14".into(),
                "-I".into(),
                "-B".into(),
                root.join("action.py").to_string_lossy().into_owned(),
                image.clone(),
            ];
            runner_args.extend(arguments);
            let runtime = crate::adapters::flux_checks::staging::select_runtime(
                &profile.runtime_bundles,
                &image,
                profile.container_mode,
                &reservation,
                runner_args,
            )?;
            let mut files = Vec::new();
            inventory(&root, &root, &mut files)?;
            files.sort_by(|a, b| a.path.cmp(&b.path));
            let manifest = backend::InputManifest {
                schema: "tl3.input_manifest.v1".into(),
                head: head.clone(),
                tree: tree.clone(),
                files,
            };
            let bytes = serde_json::to_vec(&manifest)?;
            let manifest_path = root.with_extension("manifest.json");
            private_create(&manifest_path, &bytes)?;
            fs::set_permissions(&manifest_path, fs::Permissions::from_mode(0o400))?;
            let suite = if generator {
                backend::Suite::Generator {
                    input_digest: contract.clone(),
                }
            } else {
                backend::Suite::WorkerTests {
                    input_digest: contract.clone(),
                }
            };
            // Consume the sole serialized production map, never the backend-access environment.
            let environment = serde_json::from_slice(
                &durable::read(&profile.job_environment).map_err(backend_error)?,
            )?;
            let request = backend::Request {
                repo: format!(
                    "{}/{}",
                    profile.delivery.repository.owner, profile.delivery.repository.name
                ),
                tree: backend::TreeIdentity {
                    head,
                    tree,
                    input_digest: durable::digest(&bytes),
                    root,
                    manifest: manifest_path,
                },
                suite,
                reservation,
                runner: runtime.runner,
                runtime: runtime.runtime,
                environment,
                scratch_root: runtime.scratch_root,
                no_progress_seconds: Some(
                    quiet_seconds
                        .try_into()
                        .map_err(|_| Error::Invalid("positive worker quiet allowance required"))?,
                ),
                walltime_seconds: walltime_seconds
                    .try_into()
                    .map_err(|_| Error::Invalid("positive worker walltime required"))?,
            };
            let key = format!(
                "worker:{}",
                durable::digest(&serde_json::to_vec(&(
                    &path,
                    &request.tree,
                    &served_tree,
                    &call
                ))?)
            );
            let owner = backend::Owner {
                owner: "worker-tools".into(),
                correlation: key.replace("worker:", "worker-"),
            };
            let flux = crate::adapters::run_backend::flux::Flux::new(options.clone())
                .map_err(backend_error)?;
            let intent = flux
                .prepare_intent(&request, &owner)
                .map_err(backend_error)?;
            let wait = WaitBinding {
                profile: path,
                owner,
                intent,
                binding: None,
                input: request.tree.clone(),
                served_tree,
                call,
            };
            let prepared = Prepared {
                wait,
                request,
                contract,
            };
            fs::create_dir_all(&profile.waits_root)?;
            let id = key.strip_prefix("worker:").expect("worker key");
            private_create(
                &profile.waits_root.join(format!("{id}.prepared.json")),
                &serde_json::to_vec(&prepared)?,
            )?;
            private_create(
                &profile.waits_root.join(format!("{id}.json")),
                &serde_json::to_vec(&prepared.wait)?,
            )?;
            response(&prepared, &profile, &options)
        }
    }
}

pub fn command(argument: Option<&str>) -> Result<()> {
    if argument == Some("schema") {
        println!(
            "{}",
            serde_json::to_string_pretty(&schemars::schema_for!(Command))?
        );
        return Ok(());
    }
    let mut raw = Vec::new();
    std::io::stdin()
        .take(durable::MAX_BYTES as u64 + 1)
        .read_to_end(&mut raw)?;
    if raw.len() > durable::MAX_BYTES {
        return Err(Error::Invalid("worker input command exceeds bound"));
    }
    let result = execute(serde_json::from_slice(&raw)?)?;
    serde_json::to_writer(std::io::stdout().lock(), &json!({"result":result}))?;
    std::io::stdout().flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn input_inventory_is_complete_immutable_and_refuses_symlinks() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("input");
        fs::create_dir(&root).unwrap();
        fs::create_dir(root.join(".git")).unwrap();
        fs::write(root.join(".git/ignored"), "git metadata").unwrap();
        fs::write(root.join("worker-input.json"), "{}").unwrap();
        fs::write(root.join("action.py"), "pass").unwrap();
        let mut files = Vec::new();
        inventory(&root, &root, &mut files).unwrap();
        assert_eq!(files.len(), 2);
        assert!(files.iter().all(|file| !file.path.starts_with(".git")));
        assert!(
            files
                .iter()
                .any(|file| file.path == Path::new("worker-input.json")
                    && file.sha256 == durable::digest(b"{}"))
        );
        assert!(
            fs::metadata(root.join("action.py"))
                .unwrap()
                .permissions()
                .readonly()
        );
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        std::os::unix::fs::symlink("worker-input.json", root.join("link")).unwrap();
        assert!(inventory(&root, &root, &mut Vec::new()).is_err());
        assert!(root.join("worker-input.json").exists());
    }

    #[test]
    fn repeat_parallel_demand_is_aggregate_and_threads_are_bounded() {
        let demand = reservation(3, 2, 4).unwrap();
        assert_eq!(demand.cpus.get(), 4);
        assert_eq!(demand.memory_bytes.get(), 8 << 30);
        assert_eq!(demand.native_threads.get(), 1);
        let demand = reservation(12, 2, 4).unwrap();
        assert_eq!(demand.native_threads.get(), 3);
        assert!(reservation(1, 0, 0).is_err());
    }
}
