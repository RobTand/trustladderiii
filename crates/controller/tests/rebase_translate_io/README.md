# Rebase translation differential

G2a's first cut audits steps 1–3. The three scratch/log observations already
return no event. Kernel delegates to the existing kernel-boundary decision;
CiFailed constructs a typed fact. Made, Reviewed, Merged and Plan construct
typed events around `head()` or `footprint()`. Those nine arms remain unchanged:
there is no independent decision to extract from them.

RebaseConflict and Rebased share a current-phase/current-generation decision in
`controller-core::translate_arms::rebase_reply`. Each arm has its own verbatim
old source and current source, compiled in separate modules. `pins.json` records
the SHA-256 of each arm and the source commit c3e6fbbb. The source guard compares
the references to the pinned blobs and the current copies to production.
Both sides compile the actual unchanged `Controller::head` body, also pinned.
No shared shim covers a changed arm or the head helper.

The recorded leaf boundary supplies the real kernel World/Node and Observation
input types. Its fake store models SELECT/INSERT interning, the intern
dictionary inside an input transaction, staged RowChange order, transaction
commit and persisted rows/dictionary. Each successful observation commits;
errors stop the tape and retain only previous committed state. Crashes occur
before and after every attempted INSERT and transaction commit, including
failed writes. Comparison includes calls, ordered staged rows, event/error
results, pending intern state and persisted intern/row state.

These arms have no store, clock, plan or snapshot reads. Intern SELECTs see the
state committed by preceding observations, and select/insert/commit failures
are separate inputs. Fixtures cover absent/present conflict rows and intern
entries, a same-SHA entry under another node, empty/nonempty file lists, missing
nodes, ignored phases/generations, all head-validation error categories, and
accepted u64 generation boundaries. Mandatory tests assert that Rebased stages
its conflict-row deletion before SHA validation or any intern access.

The generators use 512 schedules/sequences at seed 0x472a2026 and 128 accepted
sequences of up to 16 observations at seed 0x472b2026, with persisted proptest
regressions. All schedules replay every before/after write cut. Mandatory cases
execute the real shell copies; pure decision witnesses supplement them.

This is the smallest compilation seam: each complete match arm and its called
head helper. The outer translate prologue, shared findings/feedback tail and
actual PostgreSQL transaction writer remain outside this test. Persisted state
is a deterministic transactional model, not PostgreSQL storage bytes. Steps
4–5, which handle adoption/check evidence and feedback collections, are deferred
to the next cut.
