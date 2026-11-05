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
        "workers/src/tl3_workers/schemas/request.json",
        include_str!("../../../workers/src/tl3_workers/schemas/request.json"),
    ),
    (
        "workers/src/tl3_workers/schemas/postmortem-input.json",
        include_str!("../../../workers/src/tl3_workers/schemas/postmortem-input.json"),
    ),
    (
        "workers/src/tl3_workers/schemas/ingest-reply.json",
        include_str!("../../../workers/src/tl3_workers/schemas/ingest-reply.json"),
    ),
    (
        "workers/src/tl3_workers/schemas/classifier-reply.json",
        include_str!("../../../workers/src/tl3_workers/schemas/classifier-reply.json"),
    ),
    (
        "workers/src/tl3_workers/schemas/maker-reply.json",
        include_str!("../../../workers/src/tl3_workers/schemas/maker-reply.json"),
    ),
    (
        "workers/src/tl3_workers/schemas/writer-reply.json",
        include_str!("../../../workers/src/tl3_workers/schemas/writer-reply.json"),
    ),
    (
        "workers/src/tl3_workers/schemas/reviewer-input.json",
        include_str!("../../../workers/src/tl3_workers/schemas/reviewer-input.json"),
    ),
    (
        "workers/src/tl3_workers/schemas/ingest-input.json",
        include_str!("../../../workers/src/tl3_workers/schemas/ingest-input.json"),
    ),
    (
        "workers/src/tl3_workers/schemas/audit-input.json",
        include_str!("../../../workers/src/tl3_workers/schemas/audit-input.json"),
    ),
    (
        "workers/src/tl3_workers/schemas/planner-input.json",
        include_str!("../../../workers/src/tl3_workers/schemas/planner-input.json"),
    ),
    (
        "workers/src/tl3_workers/schemas/maker-input.json",
        include_str!("../../../workers/src/tl3_workers/schemas/maker-input.json"),
    ),
    (
        "workers/src/tl3_workers/schemas/writer-input.json",
        include_str!("../../../workers/src/tl3_workers/schemas/writer-input.json"),
    ),
    (
        "workers/src/tl3_workers/schemas/classifier-input.json",
        include_str!("../../../workers/src/tl3_workers/schemas/classifier-input.json"),
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

// #819: keep one named red/green case per remaining input root.
macro_rules! input_drift {
    ($test:ident, $role:literal) => {
        #[test]
        fn $test() {
            let path = concat!("workers/src/tl3_workers/schemas/", $role, "-input.json");
            let file = contracts::generated()
                .into_iter()
                .find(|f| f.path == path)
                .unwrap_or_else(|| panic!("missing generated input root: {path}"));
            let committed = COMMITTED.iter().find(|(p, _)| *p == path).unwrap().1;
            assert_eq!(file.text, committed, "{path}");
        }
    };
}
input_drift!(ingest_input_is_generated_byte_identically, "ingest");
input_drift!(audit_input_is_generated_byte_identically, "audit");
input_drift!(planner_input_is_generated_byte_identically, "planner");
input_drift!(maker_input_is_generated_byte_identically, "maker");
input_drift!(writer_input_is_generated_byte_identically, "writer");
input_drift!(classifier_input_is_generated_byte_identically, "classifier");
