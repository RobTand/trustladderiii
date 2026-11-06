//! Complete inventory, bytes and dependency-owner coverage, without prompt changes.
#[test]
fn complete_generated_inventory_and_bytes() {
    let files = tl3_controller::contract_schemas::generated();
    let names: std::collections::BTreeSet<_> = files.iter().map(|file| file.path.as_str()).collect();
    assert_eq!(names.len(), files.len(), "one generator owner per artifact");
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let schemas: std::collections::BTreeSet<_> = std::fs::read_dir(root.join(tl3_contracts::SCHEMA_DIRECTORY)).unwrap().map(|entry| format!("{}/{}",tl3_contracts::SCHEMA_DIRECTORY,entry.unwrap().file_name().to_str().unwrap())).filter(|path| path.ends_with(".json")).collect();
    assert_eq!(schemas, names.iter().filter(|path| path.starts_with(tl3_contracts::SCHEMA_DIRECTORY)).map(|path| (*path).to_owned()).collect());
    for file in files { assert_eq!(file.text,std::fs::read_to_string(root.join(&file.path)).unwrap(),"{}",file.path); }
}
