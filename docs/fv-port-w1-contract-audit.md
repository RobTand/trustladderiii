# W1 slice 1 contract audit correction

The exact slice 1 head is `564de367b8347789f745564e7e9f9461288a8012`. Its uncached all-kernel run passed 309 groups and 475 obligations:
https://app.buildbuddy.io/invocation/f35e7794-2d33-4a09-8385-ea86a62feef8

Planner support `f2a9b867` and ingest support `c5b701e3` were dev-verified by partial sweeps. Both are superseded by the all-kernel GREEN at `564de367`; neither earlier exact head is advertised GREEN.

The `reply_work` and `reply_rebase` blocks contain W1 proof-only reveals, pending owner W2 adoption. Their exact slice 1 hashes against W2 `ee6828cd` are:

| Block | W1 SHA-256 | W2 SHA-256 |
| --- | --- | --- |
| reply_work | `ed53cb4121db598739663df6d4d8ee148386a10f656e0e65374facea76382acc` | `d43e89c1290b7bdefa6e0f7be2d416efc4f2e2e19dc8471224b9fce02b80dcb2` |
| reply_rebase | `d36e0c12da26072f780d7aa3fb2715984e94d2a80b54d5931f3edb27608b71f0` | `a89592b87281116d6fe2132d8427a099d269f7e4e9867b33f4cc73343fe20bb4` |

The diffs add only `reveal(port_replies_contracts::transition_post);` inside those proof blocks. Complete diffs and hashes are retained in the builder output's `slice1-owner-reveals.diff` and `owner-contract-identity.json`.
