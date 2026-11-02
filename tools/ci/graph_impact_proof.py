"""Remote-only historical proof: graph health plus v42/native selection parity.

Run with bb remote --timeout=10m --script='python3 tools/ci/graph_impact_proof.py
--range PR:BASE:HEAD ...'. Each version generates its own hashes at both refs.
This checks real exported graphs, not fixture approximations of Bazel.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import tempfile
import urllib.request

import bazel_gate
import bazel_graph_impact
import graph_impact

OLD_URL = 'https://github.com/Tinder/bazel-diff/releases/download/v42.0.0/bazel-diff_deploy.jar'
OLD_SHA256 = '0170b70fe2f24477ab056c11f5c3240d8b45a1dca5ab73ad252c096679c5500c'


def planted(workspace):
    base = workspace._graphs['head']
    deps = workspace._deps['head']
    tests, sources = sorted(base['reach']), sorted(base['loc'])
    policy = bazel_graph_impact.baseline(base)
    cases = {
        'broad new test': ({**deps, '//graph-impact-proof:wide': sources},
                           tests + ['//graph-impact-proof:wide'], {'//graph-impact-proof:wide'}, 'reach'),
        'test-to-test': ({**deps, tests[0]: deps[tests[0]] + [tests[-1]]}, tests, set(), 'new test_to_test edge'),
        'conftest import': ({**deps, '//graph-impact-proof:conftest.py': [sources[0]]},
                            tests, set(), 'new conftest_source edge'),
        'uniform regression': ({**deps, **{test: deps[test] + sources for test in tests}},
                               tests, set(), 'u_pct rose'),
        'cycle': ({**deps, '//graph-impact-proof:a': ['//graph-impact-proof:b'],
                   '//graph-impact-proof:b': ['//graph-impact-proof:a']}, tests, set(), 'SCC grew or merged'),
    }
    records = {}
    for name, (edges, roots, changed, message) in cases.items():
        head = bazel_graph_impact.from_deps(edges, roots, sources, base['loc'])
        result = bazel_graph_impact.check(base, head, changed, policy)
        if not any(message in error for error in result['errors']):
            raise RuntimeError(f'planted {name} did not fail as expected: {result["errors"]}')
        records[name] = {'summary': result['summary'], 'errors': result['errors']}
    head = graph_impact.measure({test: set() for test in tests}, base['edges'], base['loc'])
    head['edge_groups'] = base['edge_groups']
    result = bazel_graph_impact.check(base, head, set(), policy)
    if not any('record the gain' in error for error in result['errors']):
        raise RuntimeError('unrecorded gain did not fail')
    records['unrecorded gain'] = {'summary': result['summary'], 'errors': result['errors']}
    for name, result in records.items():
        print(f'PLANTED {name}: {result["summary"]}', flush=True)
        print(result['errors'][0], flush=True)
    return records


def historical(workspace, name, base, head):
    for sha in (base, head):
        workspace.ensure_commit(sha)
    workspace.prepare_diff(base, head)
    native = workspace.bazel_diff(base, head)
    old, new = workspace._graphs['base'], workspace._graphs['head']
    changed, deleted = workspace.changes(base, head)
    changed_tests = bazel_graph_impact.changed_tests(old, new, workspace._typed_hashes['base'],
                                                    workspace._typed_hashes['head'], changed | deleted)
    result = bazel_graph_impact.check(old, new, changed_tests, bazel_graph_impact.baseline(old))
    result['mermaid'] = {}
    for target in result['explain_targets'][:2]:
        result['mermaid'][target] = workspace.run(workspace._executable, 'explain',
            '--startingHashes', workspace._hashes['base'], '--finalHashes', workspace._hashes['head'],
            '--depEdgesFile', workspace._source_edges['head'], '--target', target, '--format', 'mermaid')
    directory = Path(tempfile.mkdtemp(prefix='tl3-v42-parity-'))
    jar = directory / 'bazel-diff.jar'
    with urllib.request.urlopen(OLD_URL, timeout=120) as response:
        data = response.read()
    if hashlib.sha256(data).hexdigest() != OLD_SHA256:
        raise ValueError('v42 checksum mismatch')
    jar.write_bytes(data)
    java = Path(workspace.run(workspace.bazel, 'info', 'java-home').strip(), 'bin/java')
    for revision, sha in [('base', base), ('head', head)]:
        workspace.checkout(sha)
        workspace.run(java, '-jar', jar, 'generate-hashes', '--workspacePath', workspace.root,
                      '--bazelPath', workspace.bazel, '--excludeExternalTargets', '--includeTargetType',
                      directory / f'{revision}.json')
    output = directory / 'impacted.txt'
    workspace.run(java, '-jar', jar, 'get-impacted-targets', '--workspacePath', workspace.root,
                  '--bazelPath', workspace.bazel, '--startingHashes', directory / 'base.json',
                  '--finalHashes', directory / 'head.json', '--targetType', 'Rule', '--output', output)
    legacy = bazel_graph_impact.impacted_labels(output.read_text())
    parity = {'v42': len(legacy), 'v70': len(native), 'only_v42': sorted(legacy - native),
              'only_v70': sorted(native - legacy)}
    print(f'PR #{name}: {result["summary"]}', flush=True)
    print('SELECTION_PARITY ' + json.dumps(parity, sort_keys=True), flush=True)
    return {'pr': name, 'base_sha': base, 'head_sha': head, 'graph': result, 'parity': parity}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--range', action='append', required=True, help='PR:BASE:HEAD')
    args = parser.parse_args()
    workspace = bazel_gate.Workspace()
    original = workspace.run('git', 'rev-parse', 'HEAD').strip()
    records = []
    try:
        for value in args.range:
            records.append(historical(workspace, *value.split(':')))
        mutations = planted(workspace)
    finally:
        workspace.checkout(original)
    output = Path(os.environ['BUILDBUDDY_ARTIFACTS_DIRECTORY']) / 'graph-impact-history.json'
    output.write_text(json.dumps(records, indent=2, sort_keys=True) + '\n')
    (output.parent / 'graph-impact-planted.json').write_text(json.dumps(mutations, indent=2, sort_keys=True) + '\n')
    return int(any(record['parity']['only_v42'] or record['parity']['only_v70'] for record in records))


if __name__ == '__main__':
    raise SystemExit(main())
