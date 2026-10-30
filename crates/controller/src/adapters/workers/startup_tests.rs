use super::*;
use std::os::unix::fs::PermissionsExt;

#[test]
fn worker_credentials_survive_controller_credential_removal_and_expose_only_model_key() {
    let root = tempfile::tempdir().unwrap();
    let inherited = root.path().join("systemd-credentials");
    fs::create_dir(&inherited).unwrap();
    for name in ["model-key", "github-app.json", "postgres-password"] {
        fs::write(inherited.join(name), b"disposable fixture").unwrap();
        fs::set_permissions(inherited.join(name), fs::Permissions::from_mode(0o600)).unwrap();
    }
    let attempt = root.path().join("attempt");
    fs::create_dir(&attempt).unwrap();
    let mut request = json!({"key_file": inherited.join("model-key")});
    let mut command = Command::new("python3");
    command.args(["-c", "import json,os,sys; from pathlib import Path; r=json.load(sys.stdin); p=Path(os.environ['CREDENTIALS_DIRECTORY']); assert sorted(x.name for x in p.iterdir())==['model-key']; assert Path(r['key_file']).parent==p; assert Path(r['key_file']).read_bytes()==b'disposable fixture'"]);
    isolate_credentials(&attempt, &mut request, &mut command).unwrap();
    fs::remove_dir_all(inherited).unwrap();
    let path = Path::new(request["key_file"].as_str().unwrap());
    assert_eq!(path.metadata().unwrap().permissions().mode() & 0o777, 0o600);
    assert_eq!(path.parent().unwrap().metadata().unwrap().permissions().mode() & 0o777, 0o700);
    let mut child = command.stdin(Stdio::piped()).spawn().unwrap();
    serde_json::to_writer(child.stdin.take().unwrap(), &request).unwrap();
    assert!(child.wait().unwrap().success());
}
