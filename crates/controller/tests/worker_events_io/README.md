F1 preserves the cdacce8f parser and recorded refresh I/O schedule. F2 fixes one
quota-finality bug. `pre_fix.rs` additionally compiles the actual d8f58cc3
`parse_reply` and `reply_step`, with SHA256 pins checked by the normal suite.
The old core is independent of the changed production core. The current parser
copy is checked byte for byte against production by the existing source-pin test.

The sole intended divergence is a complete stream for which the old parser
successfully returns Quota at an earlier line because that line is byte-identical
to the final line. A valid progress prefix may precede the quota; anything,
including malformed events, may occur between the two identical quota lines.
The new parser returns exactly `event after quota ending`. All other generated
inputs must preserve the entire serialized observation or error, byte for byte.
The oracle identifies this class from the old successful Quota result and an
earlier exact copy of the final line. It does not exempt arbitrary quota streams
or arbitrary errors. F1 and F2 references must always agree.

The registered original regression checks both frozen references accept the
buggy stream and the current copy and actual production parser reject it.
Mandatory matrices cover omitted/explicit grade model, both roles, progress
prefixes, every generated event shape in the middle, malformed/blank tails,
incomplete output, and model/reset error precedence. A further 256 fixed-seed
cases vary positive full-width resets, prefixes and arbitrary middle bytes.
Existing mixed streams, arbitrary bytes, valid replies, round-trips, recorded
refresh reads and crashes before/after progress writes remain enabled.

Blank lines retain the existing parser behavior. They are invalid events, not
newly ignored whitespace. No sampling point or durable write changes in F2.
Seed: `0xF176_0202_6005`; persisted regressions: `proptest-regressions.txt`.
