"""Qualify #770 on a disposable BuildBuddy runner, never on a fleet host.

This is runner orchestration. Verifier execution and verdict parsing live in
Rust. Scratch commits are never pushed. Each Bazel invocation runs one target.
"""
import argparse
import json
import os
from pathlib import Path
import tempfile

import bazel_gate
import remote_gate

PROOF = '//crates/mergify:verus_delivery'
MODULE = 'crates/mergify/src/decide/delivery_step.rs'
SPEC = 'crates/mergify/proofs/delivery_spec.rs'
PINS = 'tools/bazel/verus_pins.bzl'


def commit(workspace, path, text):
    (workspace.root / path).write_text(text)
    workspace.run('git', '-c', 'user.name=Verus qualification', '-c',
                  'user.email=verus-qualification@invalid', 'commit', '--quiet',
                  '--no-verify', '-am', 'Verus qualification scratch change')
    return workspace.run('git', 'rev-parse', 'HEAD').strip()


def run_proof(workspace, name):
    statuses = workspace.test([PROOF], 'remote', {'ROLE': 'VERUS_QUALIFICATION'}, name,
                              extra=['--jobs=16', '--nocache_test_results'])
    log = workspace.root / 'bazel-testlogs/crates/mergify/verus_delivery/test.log'
    text = log.read_text() if log.is_file() else ''
    print(text, flush=True)
    return statuses, text


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('case', choices=['selection', 'mutant', 'empty', 'pin'])
    args = parser.parse_args()
    workspace = bazel_gate.Workspace()
    base = workspace.run('git', 'rev-parse', 'HEAD').strip()
    report = {'case': args.case, 'head': base}
    try:
        if args.case == 'selection':
            selections = {}
            for path in [MODULE, SPEC, 'docs/bazel.md']:
                workspace.checkout(base)
                head = commit(workspace, path, (workspace.root / path).read_text() + '\n')
                impacted = workspace.bazel_diff(base, head)
                workspace.checkout(head)
                changed, deleted = workspace.changes(base, head)
                decision = bazel_gate.plan(changed, deleted, impacted,
                                           workspace.used_sources(), workspace.tests())
                selections[path] = PROOF in decision['selected']
                assert selections[path] == (path != 'docs/bazel.md'), selections
            report['selected'] = selections
        elif args.case == 'pin':
            statuses, text = run_proof(workspace, 'verus-original-pin')
            assert statuses.get(PROOF) == 'PASSED' and 'VERUS_PROOF' in text, statuses
            # A URL pin can change without changing archive bytes. Both digest and
            # URL provenance are action inputs, so this must execute, not cache-hit.
            path = workspace.root / PINS
            pin = path.read_text().replace('% (VERUS_VERSION, VERUS_VERSION)',
                '% (VERUS_VERSION, VERUS_VERSION) + "?qualification=770"', 1)
            head = commit(workspace, PINS, pin)
            changed, deleted = workspace.changes(base, head)
            selected = bazel_gate.plan(changed, deleted, set(), set(), {PROOF})['selected']
            assert selected == [PROOF], selected
            # No --nocache_test_results here: changing only URL provenance must
            # invalidate the test action even though verifier binaries are equal.
            statuses = workspace.test([PROOF], 'remote', {'ROLE': 'VERUS_QUALIFICATION'},
                                      'verus-changed-pin', extra=['--jobs=16'])
            assert statuses.get(PROOF) == 'PASSED', statuses
            log = workspace.root / 'bazel-testlogs/crates/mergify/verus_delivery/test.log'
            assert 'VERUS_PROOF' in log.read_text()
            # Bazel's execution log is also retained by the outer runner. BEP
            # stays in /tmp and is reduced to cache/attempt facts before upload.
            events = [json.loads(line) for line in
                      (Path(tempfile.gettempdir()) / 'verus-changed-pin-bep.json').read_text().splitlines()]
            results = [event['testResult'] for event in events if 'testResult' in event]
            assert len(results) == 1 and not results[0].get('cachedLocally')
            assert not results[0].get('executionInfo', {}).get('cachedRemotely')
            report['pin_change_executed'] = True
        else:
            if args.case == 'mutant':
                path = workspace.root / MODULE
                commit(workspace, MODULE, path.read_text().replace(
                    '    delivered || acknowledgement', '    delivered && acknowledgement'))
            else:
                commit(workspace, 'crates/mergify/proofs/delivery.rs', 'use vstd::prelude::*;\nfn main() {}\n')
            statuses, text = run_proof(workspace, 'verus-' + args.case)
            assert statuses.get(PROOF) == 'FAILED', statuses
            if args.case == 'empty':
                assert '0 verified, 0 errors' in text and 'expected exactly 2 positive obligations' in text, text
            else:
                assert 'postcondition not satisfied' in text, text
            verdicts, failed = bazel_gate.classify([PROOF], statuses, {PROOF})
            outputs = remote_gate.outputs({'mode': 'batch-draft', 'verdicts': verdicts})
            assert outputs['rust'] == 'failure' and failed['rust'] == [PROOF]
            try:
                remote_gate.aggregate('rust', {'bazel': {'result': 'failure',
                    'outputs': outputs | {'scopes_uploaded': 'true'}}})
            except RuntimeError:
                report['required_rust_check'] = 'failure'
            else:
                raise AssertionError('required Rust check accepted failed Verus proof')
        report['passed'] = True
    finally:
        workspace.checkout(base)
        output = Path(os.environ['BUILDBUDDY_ARTIFACTS_DIRECTORY'])
        output.mkdir(parents=True, exist_ok=True)
        (output / ('verus-' + args.case + '.json')).write_text(json.dumps(report, indent=2) + '\n')
        print(json.dumps(report), flush=True)
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
