Question 20: option 1.

Narrow this request to items 1–3 of "Behaviour wanted", with their acceptance tests:
1. **Module.** Add `src/prismabuild/gpu_lease.py` with the liveness rules. A missing directory returns `[]`; a directory that can't be listed fails closed for GPU claims only.
2. **Claim fence.** Deny `gpu > 0` claims with `deferred_for_gate_gpu_lease` at both commit points, including the final recheck before the slot reservation.
3. **Offer.** While a lease is live, publish `observed_capacity.gpu = 0` with a `gate_gpu_lease` detail. Declared capacity stays unchanged.

Items 4–7 move to prismabuild#2633. Those are the `pb-state.json` publisher thread, the no-overtake invariant, the tmpfiles.d provisioning file with its install step, and the "Gate GPU leases" design section. Leave them out of this request, and leave out every acceptance test that only they satisfy (the status-file cases). The protocol section stays as the shared reference.
