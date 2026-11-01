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
    source.fetch_base(&tl3_worktrees::Remote::local(&f.path).unwrap(), None).unwrap();
    let materials = json!({
        "base_checkout": {"repository": bare, "commit": f.base},
        "repo": {"readme": "base", "file_list": ["README.md"]}
    });
    materialize(&root, &materials).unwrap();
    assert_eq!(fs::read_to_string(root.join("README.md")).unwrap(), "base\n");
    assert_eq!(crate::git_tests::git(&root, &["rev-parse", "HEAD"]), f.base);
    assert!(!root.join("partial").exists());
    fs::write(root.join("README.md"), "dirty").unwrap();
    materialize(&root, &materials).unwrap();
    assert_eq!(fs::read_to_string(root.join("README.md")).unwrap(), "base\n");
}
