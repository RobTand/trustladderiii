"""Plant known breaks and prove the gate's selection catches each one.

For every seed: commit the break on top of BASE, select tests with the same
code the gate uses (bazel-diff and bazel_gate.plan), run the selection on
remote execution, and require a failing target inside it. Runs on a Remote
Bazel runner (docs/bazel.md); it never pushes the seed commits.
"""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys

import bazel_gate

COMPILE_ERROR = '\ncompile_error!("seeded break");\n'
FAILING_TEST = ('\n#[cfg(test)]\nmod seeded_break {\n    #[test]\n    fn seeded_break() {\n'
                '        assert_eq!(1, 2, "seeded break");\n    }\n}\n')
PYTHON_RAISE = '\nraise RuntimeError("seeded break")\n'
FIXTURE = 'crates/prismabuild/tests/fixtures/queue/done/473655276a87335e1b1f82fcfec15c5b2beef4f5852b1d133e52b676bb3ec364.json'

# (place, path, mutation, a label prefix the failing selection must include)
SEEDS = [
    ('crate worktrees (compile error)', 'crates/worktrees/src/lib.rs', ('append', COMPILE_ERROR), '//crates/controller'),
    ('crate config (compile error)', 'crates/config/src/lib.rs', ('append', COMPILE_ERROR), '//crates/store'),
    ('crate store (failing assertion)', 'crates/store/src/lib.rs', ('append', FAILING_TEST), '//crates/store'),
    ('crate controller (compile error)', 'crates/controller/src/lib.rs', ('append', COMPILE_ERROR), '//crates/controller'),
    ('crate kernel (compile error)', 'crates/kernel/src/lib.rs', ('append', COMPILE_ERROR), '//crates/kernel'),
    ('crate github (failing assertion)', 'crates/github/src/lib.rs', ('append', FAILING_TEST), '//crates/github'),
    ('Python harness module', 'harness/src/tl3_harness/trees.py', ('append', PYTHON_RAISE), '//harness/tests'),
    ('worker module run by Rust tests', 'workers/src/tl3_workers/forecast.py', ('append', PYTHON_RAISE), '//crates/controller'),
    ('fixture file', FIXTURE, ('replace', '{"seeded": break'), '//crates/'),
]


# A worktrees change must reach every dependent crate's tests, including the
# controller tests that start Python workers, and nothing else.
WORKTREES_REACH = {
    'required': ['//crates/worktrees:', '//crates/config:', '//crates/store:',
                 '//crates/controller:test_test_workers'],
    'forbidden': ['//crates/kernel:', '//crates/github:', '//crates/mergify:', '//crates/prismabuild:'],
}


def reach(selected, rules=WORKTREES_REACH):
    missing = [prefix for prefix in rules['required']
               if not any(label.startswith(prefix) for label in selected)]
    extra = sorted(label for label in selected
                   if any(label.startswith(prefix) for prefix in rules['forbidden']))
    return {'missing': missing, 'unexpected': extra, 'ok': not missing and not extra}


def mutate(root, path, mutation):
    kind, text = mutation
    target = Path(root, path)
    if not target.is_file():
        raise bazel_gate.GateError(f'seed target {path} is missing; update SEEDS')
    target.write_text(target.read_text() + text if kind == 'append' else text)


def verdict(selected, statuses, expected_prefix):
    failing = sorted(label for label in selected if statuses.get(label) != 'PASSED')
    expected = [label for label in failing if label.startswith(expected_prefix)]
    return {'impacted_tests': len(selected), 'failing_selected': failing,
            'caught': bool(failing) and bool(expected)}


def table(rows):
    lines = ['| Place | Impacted tests | Failing target selected | Example failing target |',
             '|---|---:|---|---|']
    for row in rows:
        example = row['failing_selected'][0] if row['failing_selected'] else '-'
        lines.append(f"| {row['place']} | {row['impacted_tests']} | "
                     f"{'yes' if row['caught'] else 'NO'} | `{example}` |")
    return '\n'.join(lines)


def run_seed(workspace, base, place, path, mutation, expected_prefix):
    workspace.checkout(base)
    mutate(workspace.root, path, mutation)
    subprocess.run(['git', '-c', 'user.name=seed', '-c', 'user.email=seed@invalid', 'commit',
                    '--quiet', '--no-verify', '-am', f'seeded break: {place}'],
                   cwd=workspace.root, check=True)
    head = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=workspace.root, text=True).strip()
    changed, deleted = workspace.changes(base, head)
    impacted = workspace.bazel_diff(base, head)
    workspace.checkout(head)
    decision = bazel_gate.plan(changed, deleted, impacted, workspace.used_sources(), workspace.tests())
    statuses = workspace.test(decision['selected'], 'remote', {'ROLE': 'SEEDED_BREAK'}, 'seed')
    row = {'place': place, 'path': path, **verdict(decision['selected'], statuses, expected_prefix)}
    if path.startswith('crates/worktrees/'):
        row['reach'] = reach(decision['selected'])
        row['caught'] = row['caught'] and row['reach']['ok']
    return row


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--base', default='HEAD')
    args = parser.parse_args()
    workspace = bazel_gate.Workspace()
    base = subprocess.check_output(['git', 'rev-parse', args.base], cwd=workspace.root, text=True).strip()
    rows = [run_seed(workspace, base, *seed) for seed in SEEDS]
    workspace.checkout(base)
    report = table(rows)
    print(report)
    output = Path(os.environ.get('BUILDBUDDY_ARTIFACTS_DIRECTORY', workspace.root / 'gate-artifacts'))
    output.mkdir(parents=True, exist_ok=True)
    (output / 'seeded-breaks.json').write_text(json.dumps({'base': base, 'rows': rows}, indent=2))
    (output / 'seeded-breaks.md').write_text(report + '\n')
    return 0 if all(row['caught'] for row in rows) else 1


if __name__ == '__main__':
    sys.exit(main())
