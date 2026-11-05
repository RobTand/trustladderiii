//! The extracted contract crate still renders the committed schema (#741, #819).

use tl3_contracts as contracts;

const FIX: &str = "bazel run --config=remote //crates/contracts:tl3-contracts-schemas";

// The committed generated files, compiled in so the check stays hermetic.
const COMMITTED: &[(&str, &str)] = &[
    (
        "workers/src/tl3_workers/schemas/check-report.json",
        include_str!("../../../workers/src/tl3_workers/schemas/check-report.json"),
    ),
    (
        "workers/src/tl3_workers/schemas/decision-context.json",
        include_str!("../../../workers/src/tl3_workers/schemas/decision-context.json"),
    ),
    (
        "workers/src/tl3_workers/schemas/planner-reply.json",
        include_str!("../../../workers/src/tl3_workers/schemas/planner-reply.json"),
    ),
];

#[test]
fn generated_schemas_match_committed() {
    let generated = contracts::generated();
    assert_eq!(
        generated
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>(),
        COMMITTED.iter().map(|(path, _)| *path).collect::<Vec<_>>(),
        "every generated file is committed and compiled into this test"
    );
    for (file, (path, committed)) in generated.iter().zip(COMMITTED) {
        if file.text == *committed {
            continue;
        }
        let line = file
            .text
            .lines()
            .zip(committed.lines())
            .position(|(rendered, written)| rendered != written)
            .unwrap_or_else(|| file.text.lines().count().min(committed.lines().count()));
        panic!(
            "{path} no longer matches the Rust contract types (first difference at line {}: \
             rendered {:?}, committed {:?}). Never edit it by hand; regenerate with: {FIX}",
            line + 1,
            file.text.lines().nth(line),
            committed.lines().nth(line),
        );
    }
}
