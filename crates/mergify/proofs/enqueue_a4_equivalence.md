# A4 enqueue Kani-to-Verus equivalence

Base: B2 `504f10b0`, with rust-fv `9a658a8e32` integrated. The reference harnesses are the unmodified `enqueue_proofs.rs` at B2. All inputs retain their full typed domains. The new contracts use the real `Vec<Effect>` rather than Kani's three-slot substitute; there is no unwind or temporal bound. Production bodies are selected explicitly, never replaced by a model or trusted helper.

## enqueue_ack_sound

| Kani assertion | Verus replacement in enqueue_ack_sound_spec.rs |
|---|---|
| `first(effects) == expected` | `contract`: positive length and `effects@[0] == expected(old, read)` |
| `next.confirmed == state.confirmed || expected is Confirmed` | Identical equality in `contract`, including conflict completions |
| Refused: length is 2 | Refused branch: `effects@.len() == 2` |
| Refused: effect 1 is Permanent(Refused) | Refused branch: exact second effect |
| New refusal: length is 3 | `expected == SaveRecovery` branch: exact length |
| New refusal: effect 1 is SaveIntent | Same branch: exact second effect |
| New refusal: effect 2 is Permanent(Refused) | Same branch: exact third effect |
| New refusal: next.intent is Refused | Same branch: exact next intent |
| Otherwise length is 1 | Final branch: exact length |

The independent `expected` oracle preserves the original branch order: retained refusal; hold; missing/new refusal response; missing/pending reruns; stale head; current-head conflict; missing person evidence; acknowledged state; then issue evidence. Progress acknowledgement requires prior confirmation, person removal, merged queue, or the current head with active queue evidence or changed terminal evidence and an intent without requeue. The pre-enqueue conflict completion is a separate original outcome.

`enqueue_ack_sound_helpers.rs::outcome` ports the independent issue sub-oracle, and its `issue` contract is proved against the actual `reconcile_issue` body. It preserves both independent recovery samples, repaired-head reads, dequeue reads, and applied-marker reads. It does not use a production predicate to compute the expected result.

| Original cover | Constructive witness in enqueue_ack_sound_witness.rs |
|---|---|
| Recovery appears between reads | `recovery_appears_between_reads`: initial recovery absent, repair false, later recovery present, Pending |
| Await acknowledgement | Same witness proves Pending |
| No write applied | `absent`: Absent after applied=false |
| Changed FailedCi terminal acknowledges, previously unconfirmed | `changed_terminal`: exact progress completion and confirmed next state |
| Pre-enqueue conflict | `conflict`: exact conflict completion and confirmed next state |

## enqueue_stale_head

| Kani assertion | Verus replacement in enqueue_stale_head_spec.rs |
|---|---|
| Every stale effect is not Write | Universal assertion over every actual effect |
| Every stale effect is not Confirmed | Same universal assertion excludes both Confirmed variants |
| Eligible stale Reconcile's first effect is Retryable | Exact original gate and first-effect assertion, with positive length |

The stale predicate covers Reconcile, LabelRead, RerunRead and DispatchCommand exactly as before. It excludes merged/already-confirmed Reconcile and all other events. The Retryable gate retains refusal, hold, rerun and refusal-read precedence. An unconditional ban on Permanent would change the old property: a stale event with a prior refused intent still returns Permanent(Refused). A4 preserves that behavior and does not claim the stronger ban.

| Original cover | Constructive witness in enqueue_stale_head_witness.rs |
|---|---|
| Stale reconcile retries | `reconcile_covers`, first call |
| Held stale head waits | `reconcile_covers`, second call; exact hold outcome is also in the contract |
| Stale command, no refused intent | `command_cover`; proves the stale event, no refused intent, and no Write |

## enqueue_dequeue_fresh

| Kani assertion | Verus replacement |
|---|---|
| `pending == retry && !concluded && (!labeled || !fresh)` | Production `dequeue_pending` postcondition, plus `witness::equivalence` over all original arbitrary inputs and the independent `dequeue_contract::expected` oracle |

`fresh` requires a labeled event. If a queued ID exists, the dequeue event ID must be strictly greater. Otherwise, a completion timestamp must exist and the dequeue timestamp must be greater or equal. Missing evidence is never fresh. The timestamp comparison is lexicographic over the original Time fields, including all i64 seconds and u32 nanoseconds; no nonnegative-time or normal-nanosecond assumption is added.

| Original cover | Constructive witness in enqueue_dequeue_fresh_witness.rs |
|---|---|
| Pending without label | `waiting_for_label` |
| Pending label from old cycle | `old_cycle`: equal IDs remain stale despite a newer timestamp |
| Fresh event finishes dequeue | `fresh_event`: equal negative completion time is fresh; one earlier nanosecond is stale |

The approved A4 production hunk only annotates LabelEvent/dequeue_pending and rewrites its Option closures and derived Time comparison equivalently. `tests/dequeue_reference.rs.txt` holds the verbatim old struct and function from rust-fv. The appended proptest calls both real versions with independent Option evidence, a fixed seed and persisted regression file. The explicit boundary test covers None/Some combinations, negative/extreme seconds, equal seconds with different nanoseconds, leap nanoseconds, and strict-ID boundaries.
