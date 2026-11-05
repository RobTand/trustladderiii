//! CC-E: completion decisions, original-key selection, and store table conformance.
use proptest::prelude::*;
use proptest::test_runner::{Config, FileFailurePersistence, RngSeed, TestRunner};
use serde_json::json;
use tl3_controller::executor::Completion;
use tl3_delivery::{Ack as CoreAck, State as CoreState};
use tl3_kernel::Ack;
use tl3_store::{Effect, EffectState, EffectTransition};

#[derive(Default)]
struct Staged {
    transitions: Vec<EffectTransition>,
}
mod baseline {
    include!("reference-plan.rs");
}

fn states() -> [EffectState; 7] {
    use EffectState::*;
    [
        Intended, Sent, Uncertain, Confirmed, Failed, Cancelling, Cancelled,
    ]
}
fn state(state: EffectState) -> CoreState {
    match state {
        EffectState::Intended => CoreState::Intended,
        EffectState::Sent => CoreState::Sent,
        EffectState::Uncertain => CoreState::Uncertain,
        EffectState::Confirmed => CoreState::Confirmed,
        EffectState::Failed => CoreState::Failed,
        EffectState::Cancelling => CoreState::Cancelling,
        EffectState::Cancelled => CoreState::Cancelled,
    }
}
fn stored(state: CoreState) -> EffectState {
    states()
        .into_iter()
        .find(|s| self::state(*s) == state)
        .unwrap()
}
fn ack(ack: u8) -> (Ack, CoreAck) {
    match ack % 4 {
        0 => (Ack::Confirmed, CoreAck::Confirmed),
        1 => (Ack::Retryable, CoreAck::Retryable),
        2 => (Ack::Permanent, CoreAck::Permanent),
        _ => (Ack::BodyPolicyRefused, CoreAck::BodyPolicyRefused),
    }
}
fn effect(key: &str, state: EffectState) -> Effect {
    Effect {
        key: key.into(),
        epoch: tl3_store::Epoch::try_from(1).unwrap(),
        kind: "GitHub".into(),
        payload: json!({}),
        state,
        created_revision: tl3_store::Revision::try_from(1).unwrap(),
        updated_revision: tl3_store::Revision::try_from(1).unwrap(),
    }
}
fn extracted_plan(key: &str, effect: &Effect, ack: CoreAck) -> Vec<EffectTransition> {
    let plan = tl3_delivery::completion(state(effect.state), ack);
    let mut transitions = Vec::new();
    if plan.bridge {
        transitions.push(EffectTransition {
            key: key.into(),
            from: EffectState::Uncertain,
            to: EffectState::Intended,
        });
        transitions.push(EffectTransition {
            key: key.into(),
            from: EffectState::Intended,
            to: EffectState::Sent,
        });
    }
    transitions.push(EffectTransition {
        key: key.into(),
        from: stored(plan.from),
        to: stored(plan.to),
    });
    transitions
}
fn bytes(transitions: &[EffectTransition]) -> Vec<u8> {
    serde_json::to_vec(
        &transitions
            .iter()
            .map(|t| json!({"key":t.key,"from":t.from,"to":t.to}))
            .collect::<Vec<_>>(),
    )
    .unwrap()
}

/// State-table Effects :724-734. Exhaust all 49 edges against the real store.
#[test]
fn transition_table_conforms_to_store() {
    for from in states() {
        for to in states() {
            assert_eq!(
                tl3_delivery::allows(state(from), state(to)),
                from.allows(to)
            );
        }
    }
}

/// CC-E extraction: the complete original method is retained verbatim. The only
/// changes are pure plan substitution and the separately verified key comparison.
#[test]
fn completion_shell_keeps_every_other_statement_verbatim() {
    let source = include_str!("../../src/lib.rs");
    let start = source.find("    fn prepare_completion(").unwrap();
    let end = source[start..].find("\n    fn apply(").unwrap() + start;
    let restored = source[start..end]
        .replace(
            include_str!("current-plan.rs.txt"),
            include_str!("base-plan.rs.txt"),
        )
        .replace(
            ".find(|e| tl3_delivery::key_matches(e.key.as_bytes(), key.as_bytes()))",
            ".find(|e| e.key == key)",
        );
    assert_eq!(restored, include_str!("base-completion.rs.txt"));
    assert!(include_str!("reference-plan.rs").contains(include_str!("base-plan.rs.txt")));
}

#[derive(Debug, PartialEq, Eq)]
enum Call {
    Read(Vec<u8>),
    Commit(Vec<u8>),
}
#[derive(Debug, PartialEq, Eq)]
struct Outcome {
    calls: Vec<Call>,
    result: Vec<u8>,
    persisted: Vec<u8>,
}

// Completion has two distinct pending-key reads: the enqueue check and the
// consumption transaction. Each can change independently. No helper, clock or
// store access moves across the substitution, checked by the source test above.
fn run(
    reference: bool,
    reads: &[u8; 2],
    requested: &str,
    other: &str,
    ack_id: u8,
    crash: u8,
) -> Outcome {
    let mut calls = Vec::new();
    let persisted = b"pre-transaction".to_vec();
    for (index, sample) in reads.iter().enumerate() {
        let candidate = if sample & 1 == 0 { requested } else { other };
        calls.push(Call::Read(candidate.as_bytes().to_vec()));
        let matches = if reference {
            candidate == requested
        } else {
            tl3_delivery::key_matches(candidate.as_bytes(), requested.as_bytes())
        };
        if !matches {
            return Outcome {
                calls,
                result: b"missing key".to_vec(),
                persisted,
            };
        }
        if index == 0 {
            calls.push(Call::Commit(b"queued completion".to_vec()));
            if crash == 1 {
                return Outcome {
                    calls,
                    result: b"crash".to_vec(),
                    persisted: b"queued completion".to_vec(),
                };
            }
        }
    }
    let (ack, core_ack) = ack(ack_id);
    let completion = Completion {
        ack,
        ..Completion::confirmed(vec![])
    };
    let effect = effect(requested, states()[(reads[1] as usize / 2) % 7]);
    let transitions = if reference {
        baseline::reference_plan(requested, &effect, &completion)
    } else {
        extracted_plan(requested, &effect, core_ack)
    };
    let result = bytes(&transitions);
    calls.push(Call::Commit(result.clone()));
    Outcome {
        calls,
        result: if crash == 2 {
            b"crash".to_vec()
        } else {
            result.clone()
        },
        persisted: result,
    }
}

/// CC-E completion extraction: varying both reads, every transaction write crash,
/// and byte-identical staged transitions and persisted transaction payloads.
#[test]
fn changing_key_reads_and_completion_writes_agree() {
    let output = std::path::PathBuf::from(
        std::env::var_os("TEST_UNDECLARED_OUTPUTS_DIR").unwrap_or_else(|| std::env::temp_dir().into_os_string()),
    );
    let output = output.join("delivery-completion-regressions.txt");
    std::fs::write(&output, include_bytes!("completion-regressions.txt")).unwrap();
    let path = Box::leak(output.to_string_lossy().into_owned().into_boxed_str());
    let mut runner = TestRunner::new(Config {
        cases: 128,
        rng_seed: RngSeed::Fixed(73006),
        failure_persistence: Some(Box::new(FileFailurePersistence::Direct(path))),
        ..Config::default()
    });
    runner
        .run(
            &(
                any::<[u8; 2]>(),
                any::<u8>(),
                "[a-z0-9:]{0,100}",
                "[a-z0-9:]{0,100}",
            ),
            |(reads, ack, key, other)| {
                for crash in 0..=2 {
                    prop_assert_eq!(
                        run(false, &reads, &key, &other, ack, crash),
                        run(true, &reads, &key, &other, ack, crash)
                    );
                }
                Ok(())
            },
        )
        .unwrap();
}
