#!/usr/bin/env bash
# Development execution must be admitted through PrismaBuild.
set -euo pipefail
cd "$(dirname "$0")/../../.."
cargo test --locked -p tl3-config repo_conventions
cargo test --locked -p tl3-worktrees --test branch_template
pull=$(mktemp)
trap 'rm -f "$pull"' EXIT
TL3_REPO_CONVENTIONS_PULL="$pull" cargo test --locked -p tl3-controller --test publishing -- repo-conventions
python3 - "$pull" <<'PY'
import json
from pathlib import Path
import sys
import types

fixture = Path('crates/controller/tests/fixtures/pb-issue-link/issue_link.py.txt')
policy = types.ModuleType('pb_issue_link')
sys.modules[policy.__name__] = policy
exec(compile(fixture.read_text(), str(fixture), 'exec'), policy.__dict__)
pull = json.loads(Path(sys.argv[1]).read_text())
assert policy._BRANCH.fullmatch(pull['head']['ref'])
assert policy.referenced_issues(pull['body'], 'RobTand/prismabuild') == (42,)
verdict = policy.ordinary_decision(pull, 'RobTand/prismabuild', {42: {'number': 42}})
assert verdict.allowed, verdict.reason
pull['body'] = 'Refs #43'
assert not policy.ordinary_decision(pull, 'RobTand/prismabuild', {43: {'number': 43}}).allowed
print('PB repo policy accepted the published branch and issue body; mismatched issue rejected')
PY
