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
