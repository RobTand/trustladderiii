//! P5: real worker integration and replay drift run on the controller graph.
#[test]
fn p5_recordings_match_real_rust() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let output = std::process::Command::new(root.join("workers/.venv/bin/python"))
        .arg(root.join("workers/tests/checker_stub.py"))
        .arg("--p5-check")
        .arg(root.join("crates/controller/example_p5-vectors"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn real_worker(module: &str, index: usize, total: usize) {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let output = std::process::Command::new(root.join("workers/.venv/bin/python"))
        .arg("-c").arg(r#"
import pathlib, sys, pytest
sys.path.insert(0, str(pathlib.Path(sys.argv[1]).parent))
index, total = map(int, sys.argv[2:])
class Partition:
    def pytest_collection_modifyitems(self, config, items):
        rank = {name: position for position, name in enumerate(sorted(item.nodeid for item in items))}
        selected = [item for item in items if rank[item.nodeid] % total == index]
        config.hook.pytest_deselected(items=[item for item in items if item not in selected])
        items[:] = selected
raise SystemExit(pytest.main([sys.argv[1], '-v'], plugins=[Partition()]))
"#)
        .arg(root.join(format!("workers/tests/test_{module}.py")))
        .args([index.to_string(), total.to_string()])
        .env("TEST_REAL_CHECKER", "1").output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout.contains(" passed"), "no assertions ran: {stdout}");
}
macro_rules! scenario {
    ($($name:ident => ($module:literal, $index:literal, $total:literal)),*) => {
        $(#[test] fn $name() { real_worker($module, $index, $total); })*
    };
}
scenario!(
    quota => ("quota", 0, 1),
    maker_validation_0 => ("maker_validation", 0, 2),
    maker_validation_1 => ("maker_validation", 1, 2),
    maker_0 => ("maker", 0, 3),
    maker_1 => ("maker", 1, 3),
    maker_2 => ("maker", 2, 3),
    workers_0 => ("workers", 0, 5),
    workers_1 => ("workers", 1, 5),
    workers_2 => ("workers", 2, 5),
    workers_3 => ("workers", 3, 5),
    workers_4 => ("workers", 4, 5),
    postmortem_0 => ("postmortem", 0, 2),
    postmortem_1 => ("postmortem", 1, 2)
);
