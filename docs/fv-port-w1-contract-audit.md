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

## Slice 2 exception and shared admission composition

The sub-lead granted "sub-lead exception 11-06 s2" for proof-only additions in
`reply_work`, `reply_rebase`, `admitted_reply` and shared `accepted`. The owner
comparison is now W2 `2e0d4315dddaa8f36fa9abcf99275aab134f01da`. The output
`owner-contract-identity.json` records each complete block's owner/W1 hashes,
exact diff, unchanged signature and contract, and identical executable tokens.
The original slice-1 exception records remain separate.

W2's green review-only split is merged locally at `ba5fbdf09`. Its
`unselected` lemma remains byte-identical to the owner, with the sole
`requires !selected(reply)` premise and only `review_transition_post` as its
postcondition. W1's worker conjunct remains outside that review-only relation
in the existing opaque combined `transition_post`.

The W1-owned `port_replies_maker::unselected_maker` combines the actual maker
`transition_post` with W2's review-only fact. Accepted MakerNeedsFiles admission
first folds the real `reply_files` handler's `files_post` result into the maker
relation, then calls that helper. No W2 signature, precondition, postcondition
or executable token changes in this W1 repair. W2's imported stopping-release
refactor is byte-identical to the owner's published code.

This local repair is unverified while the sub-lead's BuildBuddy hold is active.
No new proof, test or mutant run has been submitted. The earlier red
`b5c7d4a5` has its own real-postcondition failure evidence at
https://app.buildbuddy.io/invocation/82ac257d-f90d-4ab5-9413-7dbdd41cd35e.
The prior 333-group sweep belongs only to `756acc7524`, whose shared
precondition narrowing was rejected by review. Slice 1 remains accepted at
`564de367`; verification of both slices at this new head awaits the full
uncached kernel sweep and affected mutants after the hold lifts.

## #832-A main integration, local and unverified

The next local integration imports main
`6a1f337adc6df8b1b8c4de135f4c96973389457e`, including Rob's #832-A rule at
`2d4157418`. A second AuditDisagrees in Auditing stops with
`ReviewStop::AuditDisagreement`, preserves credit, question counter and the
entire outbox, and leaves no question. The first disagreement still returns
to Ingesting. Both branches retain the saturating audit counter.

The approved stop statements are kept verbatim inside the existing private
audit helper. After helper inlining, the ingest executable tokens match the
unextracted main handler exactly. Its equivalence reference now copies that
same main body. The generator includes the new review-stop variant.

Main's #832-A clauses on `reply_ingest`, `on_reply` and `reply_transition` are
preserved byte for byte. The new W1 `audit_stop_reply` proof forwards the
combined relation into the main clause on `on_reply`; W2's `admitted_reply`
signature and contract remain unchanged. Owner identity records separate the
imported main clause from W1's proof-only forwarding.

The shared W1 rule with fail-fast off permits review-stop changes only to
PublicationBody, AuditDisagreement on the second audit disagreement, or
PlanCantMeet for a plan-check ending. Exact audit fields, output and counter
relations now describe #832-A. Cover cases 8 and 9 construct consistent,
settled original-domain worlds and execute real apply for the spent repair
with fail-fast off and on. The existing first-disagreement cover is retained.

#860 has not landed in the imported main head. Its later audit-disagreement
retirement must replace W1's retirement entry when it lands. All previous
GREEN evidence remains evidence for its original pre-integration head only.
No remote runs or pushes are made during the BuildBuddy hold.
