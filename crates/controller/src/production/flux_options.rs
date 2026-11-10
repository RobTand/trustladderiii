use crate::run_backend::{HostBootstrap, Options};

/// Explicit job profile, separate from native CLI access and ambient environment.
pub fn flux_job_environment() -> std::collections::BTreeMap<String, String> {
    [
        ("HOME", "/home/rob"),
        ("XDG_RUNTIME_DIR", "/run/user/1000"),
        ("PATH", "/usr/bin:/bin"),
    ]
    .map(|(name, value)| (name.to_owned(), value.to_owned()))
    .into()
}

/// Shared controller/worker policy; native operations resolve the current INSTANCE locator.
pub fn flux_options(config: &tl3_config::Flux) -> Options {
    let environment = config.environment.clone();
    Options {
        executable: config.executable.clone(),
        arguments: Vec::new(),
        environment,
        instance_file: config.instance_file.clone(),

        journal_root: config.output_root.join(".run-backend"),
        bootstrap: HostBootstrap {
            cli: config.backend_cli.clone(),
            config: config.output_root.join(".run-backend/options.json"),
        },
    }
}

/// Publish the exact options consumed by the controller and trusted host launcher.
pub fn provision_flux_options(
    config: &tl3_config::Flux,
    ownership: &mut crate::Ownership,
) -> crate::Result<Options> {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::{
        fs::{self, File, OpenOptions},
        io::Write,
        os::unix::fs::OpenOptionsExt,
    };
    static NEXT: AtomicU64 = AtomicU64::new(0);

    ownership.ensure_held()?;
    let options = flux_options(config);
    // Check each existing ancestor before creating a directory or replacing access settings.
    for ancestor in options.journal_root.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(metadata) if !metadata.is_dir() || metadata.file_type().is_symlink() => {
                return Err(crate::Error::Invalid(
                    "Flux bootstrap directories must be real directories",
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    fs::create_dir_all(&options.journal_root)?;
    // Both consumers read the same durable job profile; neither derives it from access settings.
    for (destination, bytes) in [
        (
            options.journal_root.join("job-environment.json"),
            serde_json::to_vec(&flux_job_environment())?,
        ),
        (
            options.bootstrap.config.clone(),
            serde_json::to_vec(&options)?,
        ),
    ] {
        let temporary = options.journal_root.join(format!(
            ".options-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let result = (|| -> crate::Result<()> {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&temporary)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            ownership.ensure_held()?;
            fs::rename(&temporary, &destination)?;
            File::open(&options.journal_root)?.sync_all()?;
            if let Some(parent) = options.journal_root.parent() {
                File::open(parent)?.sync_all()?;
            }
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result?;
    }
    Ok(options)
}
