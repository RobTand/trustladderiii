# W1 slice 1 contract audit correction

The exact slice 1 head is `564de367b8347789f745564e7e9f9461288a8012`. Its uncached all-kernel run passed 309 groups and 475 obligations:
https://app.buildbuddy.io/invocation/f35e7794-2d33-4a09-8385-ea86a62feef8

Planner support `f2a9b867` and ingest support `c5b701e3` were dev-verified by partial sweeps. Both are superseded by the all-kernel GREEN at `564de367`; neither earlier exact head is advertised GREEN.

The `reply_work` and `reply_rebase` blocks contain W1 proof-only reveals under the recorded sub-lead exception. Their exact slice 1 hashes against W2 `ee6828cd` are:

| Block | W1 SHA-256 | W2 SHA-256 |
| --- | --- | --- |
| reply_work | `ed53cb4121db598739663df6d4d8ee148386a10f656e0e65374facea76382acc` | `d43e89c1290b7bdefa6e0f7be2d416efc4f2e2e19dc8471224b9fce02b80dcb2` |
| reply_rebase | `d36e0c12da26072f780d7aa3fb2715984e94d2a80b54d5931f3edb27608b71f0` | `a89592b87281116d6fe2132d8427a099d269f7e4e9867b33f4cc73343fe20bb4` |

The recorded label is "sub-lead exception 11-06: proof-only reveal of W1 opaque relation; executable tokens identical".

The diffs add only `reveal(port_replies_contracts::transition_post);` inside those proof blocks. Complete diffs and hashes are retained in the builder output's `slice1-owner-reveals.diff` and `owner-contract-identity.json`.

## Slice 2 exception and shared admission boundary

The sub-lead granted "sub-lead exception 11-06 s2" for proof-only additions in
`reply_work`, `reply_rebase`, `admitted_reply` and shared `accepted`. The owner
comparison is W2 `8be6a5af1f5cf8a98478409eec80da37f27eaf8c`. The output
`owner-contract-identity.json` records each complete block's owner/W1 hashes,
exact diff, unchanged signature and contract, and identical executable tokens.
The original slice-1 exception records remain separate.

W2's `unselected` lemma is restored byte for byte, including its sole
`requires !selected(reply)` premise. It now fails its postcondition because
shared `transition_post` contains the W1 exact worker conjunct. This is an API
conflict, not a missing reveal: arbitrary worlds with a MakerDone reply satisfy
that premise without satisfying the maker result. A new W1 helper cannot
establish the missing result from those inputs. The same issue also reaches
the slice-1 worker conjunct.

Remote restoration probe, real postcondition failure:
https://app.buildbuddy.io/invocation/128575b8-57a1-4bf3-bcc6-51887263168f

This repair is RED pending a W2-owned shared API decision. The previous
333-group proof evidence remains evidence for `756acc7524`, whose shared
precondition narrowing was rejected by review. It does not qualify this repair.
No new full sweep or mutant pass is claimed at the restored head.
