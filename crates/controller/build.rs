use std::{env, error::Error, fs, path::PathBuf, process::Command};

fn main() -> Result<(), Box<dyn Error>> {
    println!("cargo::rustc-check-cfg=cfg(loom)");
    if env::var_os("CARGO_FEATURE_LOOM").is_some() {
        println!("cargo::rustc-cfg=loom");
    }
    packaged(
        "schemas",
        "json",
        "PACKAGED_SCHEMAS",
        "worker_schemas.rs",
        true,
    )?;
    // Built-in role prompts (PRD #327, C5), named by role.
    packaged(
        "prompts",
        "md",
        "PACKAGED_PROMPTS",
        "worker_prompts.rs",
        false,
    )?;
    release_tree()?;
    Ok(())
}

/// Embed each file of a workers package directory by absolute path, so builds
/// that fetch the controller crate alone can supply the files beside it.
fn packaged(
    folder: &str,
    extension: &str,
    constant: &str,
    output: &str,
    with_extension: bool,
) -> Result<(), Box<dyn Error>> {
    let directory = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?)
        .join("../../workers/src/tl3_workers")
        .join(folder);
    println!("cargo:rerun-if-changed={}", directory.display());
    let mut paths = fs::read_dir(&directory)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()?;
    paths.sort();
    let mut source = format!("const {constant}: &[(&str, &str)] = &[\n");
    for path in paths {
        if path.extension().is_some_and(|ext| ext == extension) {
            let name = if with_extension {
                path.file_name()
            } else {
                path.file_stem()
            }
            .ok_or("packaged file needs a name")?;
            source.push_str(&format!(
                "({:?}, include_str!({:?})),\n",
                name.to_str().ok_or("packaged filename must be UTF-8")?,
                path.to_str().ok_or("packaged path must be UTF-8")?,
            ));
        }
    }
    source.push_str("];\n");
    fs::write(PathBuf::from(env::var("OUT_DIR")?).join(output), source)?;
    Ok(())
}

/// Build the source tree's Git hash into tl3d as its release identity (Q51).
/// PrismaBuild snapshots wrap the checkout in their own commit, so the tree,
/// not the commit, names the source. Without Git, tl3d uses its own digest.
fn release_tree() -> Result<(), Box<dyn Error>> {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?);
    let git = |args: &[&str]| {
        Command::new("git")
            .args(args)
            .current_dir(&manifest)
            .output()
            .ok()
            .filter(|output| output.status.success())
            .and_then(|output| String::from_utf8(output.stdout).ok())
            .map(|text| text.trim().to_owned())
    };
    let Some(tree) = git(&["rev-parse", "--verify", "HEAD^{tree}"]) else {
        return Ok(());
    };
    // Rebuild when HEAD or the branch it names moves.
    let mut watched = vec!["HEAD".to_owned(), "packed-refs".to_owned()];
    watched.extend(git(&["symbolic-ref", "-q", "HEAD"]));
    for name in watched {
        if let Some(path) = git(&["rev-parse", "--git-path", &name]) {
            println!("cargo:rerun-if-changed={}", manifest.join(path).display());
        }
    }
    println!("cargo:rustc-env=TL3_RELEASE_TREE={tree}");
    Ok(())
}
