"""TL3's adapter for bazel-diff dependency exports. No second graph query."""
import json
from pathlib import Path
import subprocess

import graph_impact

POLICY = 'tools/ci/graph-impact-baseline.json'
ALLOWLIST = 'tools/ci/graph-impact-allowlist.json'


def canonical(label):
    return label.removeprefix('@@') if label.startswith('@@//') else label.removeprefix('@') if label.startswith('@//') else label


def path(label):
    return label.removeprefix('//').replace(':', '/').lstrip('/')


def hashes(data):
    return {canonical(label): value for label, value in data.get('hashes', data).items()}


def source_labels(data):
    sources = set()
    for label, value in hashes(data).items():
        kind = value.get('type') if isinstance(value, dict) else value.split('#', 1)[0]
        file = Path(path(label))
        if (label.startswith('//') and kind == 'SourceFile' and file.suffix in ('.py', '.rs')
                and not any(part in ('tests', 'fixtures', 'testdata') for part in file.parts)
                and not file.name.startswith('test_') and file.name != 'conftest.py'):
            sources.add(label)
    if not sources:
        raise ValueError('bazel-diff export contains no production source-file labels')
    return sources


def impacted_labels(output):
    if output.lstrip().startswith('['):
        values = json.loads(output)
        if any(not isinstance(value, dict) or not isinstance(value.get('label'), str) for value in values):
            raise ValueError('malformed impacted-target distances')
        return {canonical(value['label']) for value in values}
    return {canonical(label) for label in output.split()}


def load(deps_file, hashes_file, tests, root):
    deps = {canonical(label): [canonical(dep) for dep in values]
            for label, values in json.loads(Path(deps_file).read_text()).items()}
    source_hashes = json.loads(Path(hashes_file).read_text())
    sources = source_labels(source_hashes)
    loc = {label: len((Path(root) / path(label)).read_bytes().splitlines()) for label in sources}
    graph = graph_impact.from_deps(deps, tests, sources, loc)
    return graph, deps, hashes(source_hashes)


def crate_report(deps, tests, crates):
    """Count tests that read a crate, including through its sources filegroup."""
    crates, tests = sorted(crates), sorted(tests)
    closures = {test: graph_impact.closure(deps, test) for test in tests}
    markers = {crate: {crate, crate.split(':')[0] + ':sources'} for crate in crates}
    fanout = {crate: sum(bool(reach & markers[crate]) for reach in closures.values()) for crate in crates}
    edges = {(crate, dep) for crate in crates for dep in deps.get(crate, ()) if dep in crates}
    components = graph_impact.cycles(edges)
    # Rank real existing edges by how many (test, source) pairs disappear if cut.
    candidates = []
    all_markers = {marker for group in markers.values() for marker in group}
    for consumer, values in sorted(deps.items()):
        if not consumer.startswith('//'):
            continue
        for dependency in sorted(values):
            if dependency not in all_markers:
                continue
            touched = [test for test, reach in closures.items() if consumer in reach]
            if not touched:
                continue
            modified = {**deps, consumer: [dep for dep in values if dep != dependency]}
            removed = sum(len(closures[test] - graph_impact.closure(modified, test)) for test in touched)
            if removed:
                candidates.append({'consumer': consumer, 'dependency': dependency,
                                   'lost_reach_nodes': removed, 'tests': len(touched)})
    return {'fanout': fanout, 'largest_cycle': components[0] if components else [],
            'top_cuts': sorted(candidates, key=lambda cut: (-cut['lost_reach_nodes'], cut['consumer'],
                                                          cut['dependency']))[:10]}


def direct_hash(value):
    if isinstance(value, dict):
        return value.get('directHash', value.get('hash'))
    return value.rsplit('~', 1)[-1]


def changed_tests(base, head, base_hashes, head_hashes, changed_files):
    changed = set(head.tests) - set(base.tests)
    deps = {}
    for consumer, dep in head.edges:
        deps.setdefault(consumer, []).append(dep)
    for test in head.tests:
        if direct_hash(head_hashes[test]) != direct_hash(base_hashes.get(test, '')):
            changed.add(test)
        package = test.split(':')[0].removeprefix('//')
        # Test sources are leaves; reach excludes them, so use the full edge closure.
        for label in graph_impact.closure(deps, test):
            file = path(label)
            if (file in changed_files and (Path(file).name.startswith('test_')
                                           or '/tests/' in file)
                    and file.startswith(package + '/')):
                changed.add(test)
    return changed


def policy_at(root, sha, file, default):
    result = subprocess.run(['git', 'show', f'{sha}:{file}'], cwd=root, capture_output=True, text=True)
    return json.loads(result.stdout) if not result.returncode else default

