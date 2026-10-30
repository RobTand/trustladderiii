use std::{collections::BTreeSet, env, fs, path::Path, process::Command};

fn main() {
    let manifest = env::var("CARGO_MANIFEST_DIR").unwrap();
    let root = Path::new(&manifest).join("../..");
    let output = Command::new("git")
        .args(["ls-files", "-z"])
        .current_dir(&root)
        .output()
        .expect("read TL3's tracked message paths");
    assert!(
        output.status.success(),
        "cannot read TL3's tracked message paths"
    );
    let index = Command::new("git")
        .args(["rev-parse", "--git-path", "index"])
        .current_dir(&root)
        .output()
        .expect("locate tracked path index");
    assert!(index.status.success(), "cannot locate tracked path index");
    let index = String::from_utf8(index.stdout).unwrap();
    println!(
        "cargo:rerun-if-changed={}",
        root.join(index.trim()).display()
    );
    let mut paths = BTreeSet::new();
    for file in output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|file| !file.is_empty())
    {
        let file = std::str::from_utf8(file).expect("TL3 path is UTF-8");
        println!("cargo:rerun-if-changed={}", root.join(file).display());
        let mut path = Path::new(file);
        loop {
            let name = path.to_str().unwrap();
            if name.contains('/') {
                paths.insert(name.to_owned());
            }
            let Some(parent) = path.parent() else {
                break;
            };
            if parent.as_os_str().is_empty() {
                break;
            }
            path = parent;
        }
    }
    fs::write(
        Path::new(&env::var("OUT_DIR").unwrap()).join("message_paths.rs"),
        format!("&{:?}", paths.into_iter().collect::<Vec<_>>()),
    )
    .unwrap();
}
