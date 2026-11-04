# #783 incremental design

The controller owns immutable-store snapshots and passes owned Worlds through the real kernel. A single event can settle other requests and produce dependency steps, so the input event's node alone cannot define the touched set. Compare typed Node values with the cached previous World after every real step; never infer that only the triggering node changed. Existing snapshot decoding/encoding and business I/O remain in their original order.

Two designs considered:

1. Expose existing kernel validators without changing their rules. Cache per-node failures, pool usage, holder counts/conflicts and duplicate-ticket counts in the controller. Revalidate changed nodes, invalid cached nodes and nodes whose global prerequisites changed. Probe settling through the existing advance operation on an isolated clone and the candidate indexes. Run the original full checker periodically and compare results. This keeps the reference authoritative and avoids copying its rule logic.
2. Keep kernel entirely private and construct small projected Worlds. Rejected: remapping dependency endpoints, admission ordering and held footprints would duplicate the rule and make invalid-world equivalence fragile.

Chosen interface: Incremental::check(&World) -> Result<(), String>; CompactStep { preparation: Delta, event: Event, after: Delta }; Delta::between(&World, &World), Delta::apply(&mut World). No kernel type or transition changes. Exposing existing per-node/dependency checking and a subset settling probe changes visibility only and executes the same existing advance rules on a clone. The full checker remains unchanged.

Replay records store input-preparation and output deltas, plus a full before baseline every BASELINE_INTERVAL kernel commits. Choose 128 to bound replay recovery to 127 intervening commits while amortizing full snapshots. The interval counter is an additive metadata row committed atomically with the world and event record. A truncated or missing delta must fail with its revision; replay starting without a baseline explicitly reports the required revision. Current store atomicity means partial delta/event writes are rolled back; crash tests exercise this through real Store::commit, not separate model writes.

Normal-path timings must include the checker cache, alarm scan, typed delta construction, serialization, row searches, Store::commit and allocations. Measure baseline commits separately and report steady-state, baseline worst-case and interval-weighted added cost without presenting the weighted result as a worst-case bound. Persisted bytes are serialized added metadata/result bytes, checked at all fixture sizes.
