# PB Kani nightly guard closure

This temporary external-tooling package retains the reviewed v4 adapter and hook
byte for byte. SOURCE.json is the original ops manifest. The `.source` suffix
keeps these external modules out of repository import discovery and linting;
the admitted fixture program explicitly lints every executable payload with the
original pinned rules. The hook retains the baseline unused-import exception.

The captain installs only pb_nightly.py.source and dev_evidence.py.source as
adjacent external modules after the tooling FF, with exact pre/post hashes.
The original Sonnet HIGH REQUEST_CHANGES remains historical; parent fixes and
captain disposition resolve its findings without claiming reviewer approval.
REVIEW-DISPOSITION.md preserves the source-specific explanation.

`//tools/ci/pb_nightly:fixture-check` is a manual PB qualification executable,
not a sandbox test. Its 44 fixtures need complete Git history, shared PB CAS,
and sealed safe-reader bytes. Run `fixture-check.py SAFE_READER_SOURCE` through
pbrun and deploy/dl380g10/exact-source.py at the committed tooling head. The
safe-reader argument is its full source, sealed into the PB action. The runner
also authenticates the genuine historical first-leaf chain and runs pinned Ruff.
Synthetic identity/caller fixtures do not claim a complete passing nightly.
The fixture program is byte-identical to the producer GREEN program.

The adapter accepts only requested HEAD = campaign HEAD = proved SHA with
all 78 authenticated leaves and the sealed measured sizing table. The exact
commit deploy guard and the absent-index GitHub fallback remain unchanged.
No fixture creates the authoritative proof index. Full matching release batch
and a fresh full78 nightly still govern deployment. K5 deletes both installed
adapter and hook together, as specified in K5-REMOVAL.md.
