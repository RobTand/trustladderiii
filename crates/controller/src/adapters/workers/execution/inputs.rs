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
pub enum Command {
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
    Activation {
        profile: PathBuf,
        image: String,
    },
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Prepared {
    wait: WaitBinding,
    request: backend::Request,
    contract: String,
}

fn profile(path: &Path) -> Result<(Profile, backend::Options)> {
    let profile: Profile = serde_json::from_slice(&durable::read(path).map_err(backend_error)?)
        .map_err(|_| Error::Invalid("worker.profile"))?;
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
    let options = serde_json::from_slice(
        &durable::read(&profile.options)
            .map_err(|_| Error::Invalid("flux.activation.backend_options"))?,
    )
    .map_err(|_| Error::Invalid("flux.activation.backend_options"))?;
    Ok((profile, options))
}

fn activation(
    profile: &Profile,
    options: &backend::Options,
) -> Result<crate::adapters::flux_checks::activation::RuntimeActivation> {
    crate::adapters::flux_checks::activation::RuntimeActivation::load_bound(
        profile.activation.as_ref(),
        &profile.runtime_bundles,
        profile.container_mode,
        &profile.staging_root,
        options,
    )
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

pub(crate) fn reservation(cpus: u32, mem_gb: u32, parallel: u32) -> Result<backend::Reservation> {
    if parallel == 0
        || parallel > 4
        || cpus == 0
        || mem_gb == 0
        || cpus.max(parallel) > 10
        || mem_gb.checked_mul(parallel).is_none_or(|total| total > 24)
    {
        return Err(Error::Invalid(
            "worker demand exceeds dl380g10 plain limit of 10 cores / 24 GiB or parallel 1..=4",
        ));
    }
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

pub fn execute(command: Command) -> Result<Value> {
    match command {
        Command::Activation {
            profile: path,
            image,
        } => {
            let (profile, options) = profile(&path)?;
            let activation = activation(&profile, &options)?;
            activation.validate_image(&image)?;
            let reservation = reservation(1, 1, 1)?;
            let runtime = crate::adapters::flux_checks::staging::select_runtime(
                &profile.runtime_bundles,
                &image,
                profile.container_mode,
                &reservation,
                vec![],
            )?;
            activation.validate_prepared(&runtime, &reservation)?;
            Ok(
                json!({"activation_sha256": profile.activation.as_ref().expect("validated activation").sha256,
                      "manifest_sha256": runtime.bundle.sha256}),
            )
        }
        Command::Allocate {
            profile: path,
            call,
        } => {
            let (profile, options) = profile(&path)?;
            activation(&profile, &options)?;
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
            let activation = activation(&profile, &options)?;
            activation.validate_image(&image)?;
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
                "--collector-operation".into(),
                "collect_worker".into(),
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
            activation.validate_prepared(&runtime, &reservation)?;
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
            backend::validate_request_environment(&environment).map_err(backend_error)?;
            let request = backend::Request {
                pass_flux_uri: profile.pass_flux_uri,
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
    fn worker_execution_schema_is_generated_from_rust() {
        let generated = serde_json::to_value(schemars::schema_for!(Command)).unwrap();
        let committed: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../../../workers/src/tl3_workers/schemas/worker-execution.json"
        ))
        .unwrap();
        assert_eq!(
            generated, committed,
            "regenerate with tl3 worker-execution schema"
        );
    }

    #[test]
    #[ignore = "PB fixture: canonical shared bundle paths and uid 1000"]
    fn direct_prepare_requires_deployment_activation_and_checks_actual_options() {
        let mut fixture = crate::adapters::flux_checks::activation::tests::Fixture::new();
        let checkout = crate::git_tests::Repo::new();
        let attempt = fixture.root.path().join("attempt");
        fs::create_dir(&attempt).unwrap();
        let path = attempt.join("execution.json");
        let options = crate::production::flux_options(&fixture.flux);
        fs::create_dir_all(options.bootstrap.config.parent().unwrap()).unwrap();
        fs::write(
            &options.bootstrap.config,
            serde_json::to_vec(&options).unwrap(),
        )
        .unwrap();
        let mut profile = Profile {
            pass_flux_uri: false,
            activation: None,
            schema: "tl3.worker_execution.v1".into(),
            attempt: attempt.clone(),
            checkout: fs::canonicalize(&checkout.path).unwrap(),
            delivery: checkout.delivery(),
            options: options.bootstrap.config.clone(),
            job_environment: options.journal_root.join("job-environment.json"),
            staging_root: fixture.flux.staging_root.clone(),
            runtime_bundles: fixture.flux.runtime_bundles.clone(),
            container_mode: fixture.flux.container_mode,
            waits_root: fixture.root.path().join("waits"),
        };
        fs::write(attempt.join("request.json"),serde_json::to_vec(&json!({"worktree":checkout.path,"inputs":{"tests":{"execution":{"profile":path}}}})).unwrap()).unwrap();
        let command = || Command::Prepare {
            profile: path.clone(),
            call: "a".repeat(64),
            served_tree: "b".repeat(40),
            contract: "c".repeat(64),
            generator: false,
            image: fixture.flux.container_image.clone(),
            cpus: 1,
            mem_gb: 1,
            parallel: 1,
            quiet_seconds: 10,
            walltime_seconds: 60,
            arguments: vec![],
        };
        fs::write(&path, serde_json::to_vec(&profile).unwrap()).unwrap();
        let error = execute(command()).unwrap_err().to_string();
        assert_eq!(error, "flux.activation");
        assert!(
            !fixture.flux.staging_root.exists(),
            "direct Prepare wrote input before refusing activation"
        );
        profile.activation = fixture.flux.activation.clone();
        fs::write(&path, serde_json::to_vec(&profile).unwrap()).unwrap();
        let result = execute(Command::Activation {
            profile: path.clone(),
            image: fixture.flux.container_image.clone(),
        })
        .unwrap();
        assert!(result.get("allowed").is_none());
        assert_eq!(
            result["activation_sha256"],
            fixture.flux.activation.as_ref().unwrap().sha256
        );
        assert!(
            execute(command())
                .unwrap_err()
                .to_string()
                .contains("worker input was not allocated")
        );
        let changed = options.clone();
        fs::write(&changed.instance_file, "local:///replacement\n").unwrap();
        fs::write(
            &options.bootstrap.config,
            serde_json::to_vec(&changed).unwrap(),
        )
        .unwrap();
        let error = execute(command()).unwrap_err().to_string();
        assert!(error.contains("worker input was not allocated"));
        assert!(!error.contains("private-changed-generation"));
        fs::write(
            &options.bootstrap.config,
            serde_json::to_vec(&options).unwrap(),
        )
        .unwrap();
        fs::write(
            &profile.job_environment,
            serde_json::to_vec(&crate::production::flux_job_environment()).unwrap(),
        )
        .unwrap();
        let allocated = execute(Command::Allocate {
            profile: path.clone(),
            call: "a".repeat(64),
        })
        .unwrap();
        let root = PathBuf::from(allocated["root"].as_str().unwrap());
        git(&root, &["init", "--quiet"]).unwrap();
        fs::write(root.join("action.py"), b"pass\n").unwrap();
        fs::write(root.join("worker-input.json"), b"{}").unwrap();
        let prepared = execute(Command::Prepare {
            profile: path.clone(),
            call: "a".repeat(64),
            served_tree: "b".repeat(40),
            contract: durable::digest(b"{}"),
            generator: false,
            image: fixture.flux.container_image.clone(),
            cpus: 1,
            mem_gb: 1,
            parallel: 1,
            quiet_seconds: 10,
            walltime_seconds: 60,
            arguments: vec![],
        })
        .unwrap();
        assert_eq!(
            prepared["submit"]["request"]["runtime"]["manifest_digest"],
            fixture.record.profiles[0].manifest_sha256
        );
        assert_eq!(prepared["submit"]["intent"]["instance"], "runtime");
        assert!(prepared["key"].as_str().unwrap().starts_with("worker:"));
        // Record profile/manifest substitution must also fail before input allocation.
        fixture.record.profiles[0].image.profile = "unqualified".into();
        fixture.seal_record();
        profile.activation = fixture.flux.activation.clone();
        fs::write(&path, serde_json::to_vec(&profile).unwrap()).unwrap();
        fs::write(
            &options.bootstrap.config,
            serde_json::to_vec(&options).unwrap(),
        )
        .unwrap();
        assert!(
            execute(Command::Prepare {
                profile: path,
                call: "a".repeat(64),
                served_tree: "b".repeat(40),
                contract: "c".repeat(64),
                generator: true,
                image: fixture.flux.container_image.clone(),
                cpus: 1,
                mem_gb: 1,
                parallel: 1,
                quiet_seconds: 10,
                walltime_seconds: 60,
                arguments: vec![],
            })
            .is_err()
        );
    }

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
        let demand = reservation(2, 2, 4).unwrap();
        assert_eq!(demand.cpus.get(), 4);
        assert_eq!(demand.memory_bytes.get(), 8 << 30);
        assert_eq!(demand.native_threads.get(), 1);
        assert_eq!(reservation(3, 2, 4).unwrap().cpus.get(), 4);
        assert_eq!(reservation(10, 2, 4).unwrap().native_threads.get(), 2);
        assert!(reservation(11, 2, 4).is_err());
        assert!(reservation(2, 8, 4).is_err());
        assert!(reservation(1, 0, 0).is_err());
    }
}
