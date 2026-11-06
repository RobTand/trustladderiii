use crate::run_backend::{HostBootstrap, Options};

/// Shared controller/worker conversion. The configured URI wins once at this boundary.
pub fn flux_options(config: &tl3_config::Flux) -> Options {
    let mut environment = config.environment.clone();
    environment.insert("FLUX_URI".into(), config.uri.clone());
    Options {
        executable: config.executable.clone(),
        arguments: Vec::new(),
        environment,
        instance_generation: config.instance_generation.clone(),
        uri: config.uri.clone(),
        journal_root: config.output_root.join(".run-backend"),
        bootstrap: HostBootstrap {
            cli: config.backend_cli.clone(),
            config: config.output_root.join(".run-backend/options.json"),
        },
    }
}

/// Publish the exact options consumed by the controller and trusted host launcher.
pub fn provision_flux_options(config: &tl3_config::Flux, ownership: &mut crate::Ownership) -> crate::Result<Options> {
    use std::{fs::{self, File, OpenOptions}, io::Write, os::unix::fs::OpenOptionsExt};
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);

    ownership.ensure_held()?;
    let options = flux_options(config);
    // Check each existing ancestor before creating a directory or replacing access settings.
    for ancestor in options.journal_root.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(metadata) if !metadata.is_dir() || metadata.file_type().is_symlink() => {
                return Err(crate::Error::Invalid("Flux bootstrap directories must be real directories"));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    fs::create_dir_all(&options.journal_root)?;
    let temporary = options.journal_root.join(format!(".options-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    let bytes = serde_json::to_vec(&options)?;
    let result = (|| -> crate::Result<()> {
        let mut file = OpenOptions::new().write(true).create_new(true).mode(0o600).open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        ownership.ensure_held()?;
        fs::rename(&temporary, &options.bootstrap.config)?;
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
    Ok(options)
}
