//! Complete inventory, bytes and dependency-owner coverage, without prompt changes.
#[test]
fn complete_generated_inventory_and_bytes() {
    let files = tl3_controller::contract_schemas::generated();
    let names: std::collections::BTreeSet<_> =
        files.iter().map(|file| file.path.as_str()).collect();
    assert_eq!(names.len(), files.len(), "one generator owner per artifact");
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let schemas: std::collections::BTreeSet<_> =
        std::fs::read_dir(root.join(tl3_contracts::SCHEMA_DIRECTORY))
            .unwrap()
            .map(|entry| {
                format!(
                    "{}/{}",
                    tl3_contracts::SCHEMA_DIRECTORY,
                    entry.unwrap().file_name().to_str().unwrap()
                )
            })
            .filter(|path| path.ends_with(".json"))
            .collect();
    assert_eq!(
        schemas,
        names
            .iter()
            .filter(|path| path.starts_with(tl3_contracts::SCHEMA_DIRECTORY))
            .map(|path| (*path).to_owned())
            .collect()
    );
    for file in files {
        assert_eq!(
            file.text,
            std::fs::read_to_string(root.join(&file.path)).unwrap(),
            "{}",
            file.path
        );
    }
}

#[test]
fn shared_role_suffix_and_generation_guidance_preserve_controller_bytes() {
    use serde_json::{Value, json};
    use tl3_contracts::role_contract::*;
    fn check<C: RoleContract>(contract: C) {
        for cpus in [1, 7] {
            let request = json!({"role":C::ROLE,"text":"","test_cpus":cpus});
            let rendered =
                tl3_controller::prompt_render::render("role", &request.to_string()).unwrap();
            let controller: Value = serde_json::from_str(&rendered).unwrap();
            assert_eq!(
                contract
                    .render(RenderOptions {
                        test_cpus: cpus,
                        generation: false
                    })
                    .unwrap()
                    .prompt_suffix,
                controller["text"],
                "{}",
                C::ROLE.as_str()
            );
        }
    }
    check(Ingest::<Value> { rules: vec![] });
    check(Audit::<Value> { rules: vec![] });
    check(Planner::<Value> { rules: vec![] });
    check(Maker::<Value> { rules: vec![] });
    check(Reviewer::<Value> { rules: vec![] });
    check(Classifier::<Value> { rules: vec![] });
    check(Writer::<Value> { rules: vec![] });
    check(Postmortem::<Value> { rules: vec![] });
    let generation: Value =
        serde_json::from_str(&tl3_controller::prompt_render::render("generation", "{}").unwrap())
            .unwrap();
    assert_eq!(generation["text"], guidance::GENERATION);
}
