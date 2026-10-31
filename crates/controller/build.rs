use std::{env, error::Error, fs, path::PathBuf};

fn main() -> Result<(), Box<dyn Error>> {
    println!("cargo::rustc-check-cfg=cfg(loom)");
    if env::var_os("CARGO_FEATURE_LOOM").is_some() {
        println!("cargo::rustc-cfg=loom");
    }
    let directory = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?)
        .join("../../workers/src/tl3_workers/schemas");
    println!("cargo:rerun-if-changed={}", directory.display());
    let mut paths = fs::read_dir(&directory)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()?;
    paths.sort();
    let mut source = String::from("const PACKAGED_SCHEMAS: &[(&str, &str)] = &[\n");
    for path in paths {
        if path.extension().is_some_and(|ext| ext == "json") {
            let name = path.file_name().ok_or("schema needs a filename")?;
            source.push_str(&format!(
                "({:?}, include_str!({:?})),\n",
                name.to_str().ok_or("schema filename must be UTF-8")?,
                path.to_str().ok_or("schema path must be UTF-8")?,
            ));
        }
    }
    source.push_str("];\n");
    fs::write(
        PathBuf::from(env::var("OUT_DIR")?).join("worker_schemas.rs"),
        source,
    )?;
    Ok(())
}
