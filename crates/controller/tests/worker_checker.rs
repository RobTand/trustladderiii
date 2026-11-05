//! #807: worker unit tests use recorded Rust facts, checked against the real core.
#[path = "worker_checker/oracle.rs"]
mod oracle;

#[test]
fn recorded_worker_verdicts_match_the_real_rust_decisions() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let output = std::process::Command::new(root.join("workers/.venv/bin/python"))
        .arg(root.join("workers/tests/checker_stub.py"))
        .arg("--vectors")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let vectors: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(vectors.as_array().is_some_and(|rows| !rows.is_empty()));
    let actual = oracle::generate(serde_json::from_value(vectors.clone()).unwrap()).unwrap();
    assert_eq!(
        serde_json::to_value(actual).unwrap(),
        vectors,
        "run checker_stub.py --generate with the checker-vectors example"
    );
}

/// #807: the committed --check workflow must run, including the Rust example.
#[test]
fn generator_check_passes_and_rejects_a_recorded_verdict_mutant() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let python = root.join("workers/.venv/bin/python");
    let stub = root.join("workers/tests/checker_stub.py");
    let oracle = root.join("crates/controller/example_checker-vectors");
    let output = std::process::Command::new(&python)
        .args([stub.as_os_str(), "--check".as_ref(), oracle.as_os_str()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let output = std::process::Command::new(&python)
        .arg("-c")
        .arg(r#"
import gzip, json, pathlib, shutil, subprocess, sys, tempfile
stub, oracle = map(pathlib.Path, sys.argv[1:])
with gzip.open(stub.with_name('checker-vectors.json.gz'), 'rt') as handle:
    corpus = json.load(handle)
row = next(row for rows in corpus.values() for row in rows)
row['response'] = {'mutant': True}
with tempfile.TemporaryDirectory() as directory:
    script = pathlib.Path(directory, 'checker_stub.py')
    shutil.copyfile(stub, script)
    script.with_name('checker-vectors.json.gz').write_bytes(gzip.compress(json.dumps({'mutant': [row]}).encode(), mtime=0))
    result = subprocess.run([sys.executable, str(script), '--check', str(oracle)], capture_output=True, text=True)
    assert result.returncode != 0 and 'Rust checker recordings drifted' in result.stderr, result.stderr
print('recorded-verdict mutant rejected')
"#)
        .args([stub, oracle])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("recorded-verdict mutant rejected"));
}

mod pb_evidence;
mod pb_parsers;
