use std::{
    env,
    error::Error,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

fn main() -> Result<(), Box<dyn Error>> {
    println!("cargo::rustc-check-cfg=cfg(loom)");
    if env::var_os("CARGO_FEATURE_LOOM").is_some() {
        println!("cargo::rustc-cfg=loom");
    }
    let workers =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR")?).join("../../workers/src/tl3_workers");
    let schemas = workers.join("schemas");
    println!("cargo:rerun-if-changed={}", schemas.display());
    let mut source = String::from("const PACKAGED_SCHEMAS: &[(&str, &str)] = &[\n");
    for path in files(&schemas, "json")? {
        let name = path.file_name().ok_or("schema needs a filename")?;
        source.push_str(&format!(
            "({:?}, include_str!({:?})),\n",
            name.to_str().ok_or("schema filename must be UTF-8")?,
            path.to_str().ok_or("schema path must be UTF-8")?,
        ));
    }
    source.push_str("];\n");
    let out = PathBuf::from(env::var("OUT_DIR")?);
    fs::write(out.join("worker_schemas.rs"), source)?;
    // Built-in role prompts (PRD #327, C5), by role. Their text goes into the
    // generated file, so a build that fetches this crate alone, such as
    // tl3-console's Bazel build, needs the files only while this script runs.
    let prompts = workers.join("prompts");
    println!("cargo:rerun-if-changed={}", prompts.display());
    let mut source = String::from("const PACKAGED_PROMPTS: &[(&str, &str)] = &[\n");
    for path in files(&prompts, "md")? {
        println!("cargo:rerun-if-changed={}", path.display());
        let role = path.file_stem().ok_or("prompt needs a filename")?;
        source.push_str(&format!(
            "({:?}, {:?}),\n",
            role.to_str().ok_or("prompt filename must be UTF-8")?,
            fs::read_to_string(&path)?,
        ));
    }
    source.push_str("];\n");
    fs::write(out.join("worker_prompts.rs"), source)?;
    let redactor = workers.join("tracing.py");
    println!("cargo:rerun-if-changed={}", redactor.display());
    fs::write(
        out.join("worker_redactor.rs"),
        format!(
            "const REDACTOR_SOURCE: &str = {:?};\n",
            fs::read_to_string(redactor)?,
        ),
    )?;
    release_tree()?;
    Ok(())
}

/// A directory's files with one extension, sorted.
fn files(directory: &Path, extension: &str) -> Result<Vec<PathBuf>, Box<dyn Error>> {
    let mut paths = fs::read_dir(directory)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()?;
    paths.retain(|path| path.extension().is_some_and(|ext| ext == extension));
    paths.sort();
    Ok(paths)
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
