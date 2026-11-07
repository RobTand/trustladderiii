use std::{env, fs, path::PathBuf};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let schemas = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?)
        .join("../../workers/src/tl3_workers/schemas");
    println!("cargo:rerun-if-changed={}", schemas.display());
    let mut paths = fs::read_dir(schemas)?.map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()?;
    paths.retain(|path| path.extension().is_some_and(|extension| extension == "json"));
    paths.sort();
    let mut source = String::from("const PACKAGED_SCHEMAS: &[(&str, &str)] = &[\n");
    for path in paths {
        source.push_str(&format!("({:?}, include_str!({:?})),\n",
            path.file_name().ok_or("schema needs a filename")?.to_str().ok_or("UTF-8 schema name")?,
            path.to_str().ok_or("UTF-8 schema path")?));
    }
    source.push_str("];\n");
    fs::write(PathBuf::from(env::var("OUT_DIR")?).join("schemas.rs"), source)?;
    Ok(())
}
