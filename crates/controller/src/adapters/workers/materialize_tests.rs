use super::*;

#[test]
fn unmaterialized_binary_does_not_block_text_snapshot() {
    let root = tempfile::tempdir().unwrap();
    materialize(
        root.path(),
        &json!({
            "file_list": ["image.png", "src/main.py", "empty.py"],
            "materials": [{"path": "empty.py", "content": ""},
                {"path": "src/main.py", "content": "hello\n"}],
        }),
    )
    .unwrap();
    assert_eq!(
        fs::read(root.path().join("src/main.py")).unwrap(),
        b"hello\n"
    );
    assert!(fs::read(root.path().join("empty.py")).unwrap().is_empty());
    assert_eq!(fs::read(root.path().join("image.png")).unwrap(), b"\0");
    assert!(!root.path().join("new.py").exists());
}

#[test]
fn missing_text_does_not_bypass_path_confinement() {
    for path in [
        "/escape",
        "../escape",
        ".git/config",
        "a//b",
        "a/./b",
        "a\\b",
    ] {
        let root = tempfile::tempdir().unwrap();
        assert!(
            materialize(root.path(), &json!({"file_list": [path]})).is_err(),
            "{path}"
        );
    }
}

#[test]
fn exploration_checkout_uses_pinned_commit_and_recovers_partial_checkout() {
    let f = crate::git_tests::Repo::new();
    let root = f.root.path().join("explore");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("partial"), "interrupted clone").unwrap();
    let bare = f.root.path().join("fetched.git");
    Worktrees::init_bare(&bare).unwrap();
    let mut source = Worktrees::open(&bare, &bare).unwrap();
    source.set_base_branch("main").unwrap();
    source
        .fetch_base(&tl3_worktrees::Remote::local(&f.path).unwrap(), None)
        .unwrap();
    let materials = json!({
        "base_checkout": {"repository": bare, "commit": f.base},
        "repo": {"readme": "base", "file_list": ["README.md"]}
    });
    materialize(&root, &materials).unwrap();
    assert_eq!(
        fs::read_to_string(root.join("README.md")).unwrap(),
        "base\n"
    );
    assert_eq!(crate::git_tests::git(&root, &["rev-parse", "HEAD"]), f.base);
    assert!(!root.join("partial").exists());
    fs::write(root.join("README.md"), "dirty").unwrap();
    materialize(&root, &materials).unwrap();
    assert_eq!(
        fs::read_to_string(root.join("README.md")).unwrap(),
        "base\n"
    );
}

#[test]
fn repository_orientation_and_ingest_replay() {
    let generated = crate::git_tests::Repo::new();
    let repositories = if let Some(root) = std::env::var_os("TL3_TEST_REPOSITORIES") {
        ["prismabuild", "prismaquant", "tessera"]
            .into_iter()
            .map(|name| (name, PathBuf::from(&root).join(format!("{name}.git"))))
            .collect::<Vec<_>>()
    } else {
        fs::create_dir_all(generated.path.join("tools/fleet")).unwrap();
        fs::create_dir_all(generated.path.join("tests")).unwrap();
        fs::write(
            generated.path.join("tools/fleet/docker"),
            "# --gpus grammar fixture\n".repeat(600),
        )
        .unwrap();
        fs::write(
            generated
                .path
                .join("tests/test_docker_shim_global_options.py"),
            "def test_options(): pass\n",
        )
        .unwrap();
        crate::git_tests::git(&generated.path, &["add", "."]);
        crate::git_tests::git(&generated.path, &["commit", "-qm", "replay fixture"]);
        vec![("prismabuild-fixture", generated.path.clone())]
    };
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../../workers/tests/fixtures/prismabuild-1968.json"
    ))
    .unwrap();
    let issue = json!({"number": fixture["number"], "title": fixture["title"], "body": fixture["body"], "author":"requester", "comments":[]});
    let root = tempfile::tempdir().unwrap();
    for (name, repository) in repositories {
        let source = Worktrees::open(&repository, &repository).unwrap();
        let head = source.head().unwrap();
        let repo = crate::production::repository_materials(&repository, head.as_str()).unwrap();
        let inputs = json!({"issue":issue,"repo":repo});
        protocol::validate_inputs("ingest", &inputs).unwrap();
        protocol::validate_inputs("audit", &json!({"issue":issue,"repo":repo,"ingest":{
            "kind":"brief","brief":"Check GPU options","goals":[{"description":"Reject invalid options","verification":"test"}],"memory_ids":[]
        }})).unwrap();
        println!(
            "ORIENTATION {}",
            json!({"repository":name,"head":head.as_str(),"file_count":repo["file_count"],"listed_files":repo["file_list"].as_array().unwrap().len(),"orientation_bytes":repo.to_string().len(),"summarized":repo["tree_summary"] != ""})
        );
        if name.starts_with("prismabuild") {
            let checkout = root.path().join("checkout");
            fs::create_dir(&checkout).unwrap();
            materialize(&checkout, &json!({"base_checkout":{"repository":repository,"commit":head.as_str()},"repo":repo})).unwrap();
            let inputs_path = root.path().join("inputs.json");
            fs::write(&inputs_path, inputs.to_string()).unwrap();
            let workers = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../workers");
            let output = Command::new(workers.join(".venv/bin/python"))
                .arg(workers.join("tools/replay_repository_ingest.py"))
                .arg(&inputs_path)
                .arg(&checkout)
                .arg(name)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            println!("REPLAY {}", String::from_utf8_lossy(&output.stdout));
        }
    }
}
