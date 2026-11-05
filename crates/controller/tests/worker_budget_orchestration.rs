//! #807: budget assertions execute with the real checker on the controller side.
//! Unit worker targets replay the same scenarios without depending on adapters.
fn real_checker(module: &str, index: usize, total: usize) {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let output = std::process::Command::new(root.join("workers/.venv/bin/python"))
        .arg("-c")
        .arg(
            r#"
import pathlib, sys, pytest
sys.path.insert(0, str(pathlib.Path(sys.argv[1]).parent))
index, total = map(int, sys.argv[2:])
class Partition:
    def pytest_collection_modifyitems(self, config, items):
        rank = {name: position for position, name in enumerate(sorted(item.nodeid for item in items))}
        selected = [item for item in items if rank[item.nodeid] % total == index]
        config.hook.pytest_deselected(items=[item for item in items if item not in selected])
        items[:] = selected
raise SystemExit(pytest.main([sys.argv[1], '-v', '--durations=5'], plugins=[Partition()]))
"#,
        )
        .arg(root.join(format!("workers/tests/test_{module}.py")))
        .args([index.to_string(), total.to_string()])
        .env("TEST_REAL_CHECKER", "1")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stdout}\n{stderr}");
    assert!(
        stdout.contains(" passed"),
        "no Python assertions ran: {stdout}"
    );
}
// Match the worker modules' existing 20-second partitions. Each assertion is
// selected exactly once, including the complete 513-turn backstop trace.
macro_rules! scenario {
    ($($name:ident => ($module:literal, $index:literal, $total:literal)),*) => {
        $(#[test] fn $name() { real_checker($module, $index, $total); })*
    };
}
scenario!(
    exploration => ("exploration", 0, 1),
    maker_budget => ("maker_budget", 0, 1),
    maker_compaction_0 => ("maker_compaction", 0, 4),
    maker_compaction_1 => ("maker_compaction", 1, 4),
    maker_compaction_2 => ("maker_compaction", 2, 4),
    maker_compaction_3 => ("maker_compaction", 3, 4),
    maker_context => ("maker_context", 0, 1),
    maker_forecast_0 => ("maker_forecast", 0, 2),
    maker_forecast_1 => ("maker_forecast", 1, 2),
    maker_reasoning => ("maker_reasoning", 0, 1),
    maker_turn_budget => ("maker_turn_budget", 0, 1),
    maker_sessions => ("maker_sessions", 0, 1),
    planner_forecast_0 => ("planner_forecast", 0, 4),
    planner_forecast_1 => ("planner_forecast", 1, 4),
    planner_forecast_2 => ("planner_forecast", 2, 4),
    planner_forecast_3 => ("planner_forecast", 3, 4),
    progress_budget_0 => ("progress_budget", 0, 3),
    progress_budget_1 => ("progress_budget", 1, 3),
    progress_budget_2 => ("progress_budget", 2, 3),
    prompt_cache => ("prompt_cache", 0, 1),
    turns => ("turns", 0, 1)
);
