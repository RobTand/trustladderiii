"""TL3's adapter for bazel-diff dependency exports. No second graph query."""
import json
from pathlib import Path
import subprocess

import graph_impact

POLICY = 'tools/ci/graph-impact-baseline.json'
ALLOWLIST = 'tools/ci/graph-impact-allowlist.json'


def closure(deps, start):
    seen, pending = set(), [start]
    while pending:
        node = pending.pop()
        if node not in seen:
            seen.add(node)
            pending.extend(deps.get(node, ()))
    return seen


def from_deps(deps, tests, sources, loc=None):
    sources, tests = set(sources), set(tests)
    if not tests or tests - deps.keys():
        raise ValueError('test targets missing from dependency export')
    if not sources:
        raise ValueError('no production source labels')
    nodes = set(deps) | {dep for values in deps.values() for dep in values}
    if sources - nodes:
        raise ValueError('source labels missing from dependency export')
    weights = {source: (loc or {}).get(source, 1) for source in sources}
    edges = {(node, dep) for node, values in deps.items() for dep in values}
    measured = graph_impact.measure({test: closure(deps, test) & sources for test in tests}, edges, weights)
    conftests = {node for node in deps if node.endswith((':conftest', ':conftest.py', '/conftest.py'))}
    measured['edge_groups']['conftest_source'] = sorted(
        f'{consumer} -> {dependency}' for consumer, dependency in edges
        if consumer in conftests and closure(deps, dependency) & sources)
    return measured


def baseline(graph):
    return graph_impact.baseline_record(graph, tolerances={'u_pct': .1, 'loc_u_pct': .1, 'scc_max': 0})


def check(base, head, changed, policy, allowlist=None, previous_allowlist=None, previous_policy=None,
          record_gains=False):
    errors = graph_impact.allowlist_errors(allowlist or {}, previous_allowlist if previous_allowlist is not None
                                          else allowlist or {})
    errors += graph_impact.baseline_errors(policy, previous_policy)
    errors += graph_impact.compare(base, head, policy, changed_tests=changed, allowlist=allowlist,
                                   record_gains=record_gains)
    added = set(map(tuple, head['edges'])) - set(map(tuple, base['edges']))
    deps = {}
    for source, target in head['edges']:
        deps.setdefault(source, []).append(target)
    consumers = {source for source, _ in added}
    targets = (set(head['reach']) - set(base['reach'])) | set(changed)
    if errors:
        targets.update(test for test in head['reach'] if closure(deps, test) & consumers)
    else:
        targets = set()
    growth = sorted(((source, n - base['dependents'].get(source, 0))
                     for source, n in head['dependents'].items() if n > base['dependents'].get(source, 0)),
                    key=lambda row: (-row[1], row[0]))
    return {'summary': graph_impact.summary(base, head, policy['reach_budget']), 'errors': errors,
            'base': base, 'head': head, 'added_edges': [list(edge) for edge in sorted(added)],
            'fanout_growth': [list(row) for row in growth], 'explain_targets': sorted(targets)}


def write_baseline(graph, previous=None):
    if previous is None:
        return baseline(graph)
    errors = graph_impact.regressions(graph, previous)
    if graph['reach_p90'] > previous['reach_budget']:
        errors.append('reach budget may not grow')
    if errors:
        raise ValueError('baseline writer refuses losses: ' + '; '.join(errors))
    return graph_impact.baseline_record(graph, previous)


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


def needs_source_edges(deps, sources):
    consumed = {canonical(dep) for values in deps.values() for dep in values}
    return bool(set(sources) - consumed)


def add_source_edges(deps, streamed_query):
    """Keep bazel-diff's rule edges; supplement only rule -> first-party source."""
    records = [json.loads(line) for line in streamed_query.splitlines() if line.strip()]
    sources = {canonical(record['sourceFile']['name']) for record in records
               if record.get('type') == 'SOURCE_FILE'}
    sources = {source for source in sources if source.startswith('//')}
    result = {canonical(label): set(map(canonical, values)) for label, values in deps.items()}
    for record in records:
        rule = record.get('rule')
        if not rule:
            continue
        label = canonical(rule['name'])
        if label in result:
            result[label].update(canonical(dep) for dep in rule.get('ruleInput', ())
                                 if canonical(dep) in sources)
    return {label: sorted(values) for label, values in result.items()}


def load(deps_file, hashes_file, tests, root):
    deps = {canonical(label): [canonical(dep) for dep in values]
            for label, values in json.loads(Path(deps_file).read_text()).items()}
    source_hashes = json.loads(Path(hashes_file).read_text())
    sources = source_labels(source_hashes)
    loc = {label: len((Path(root) / path(label)).read_bytes().splitlines()) for label in sources}
    graph = from_deps(deps, tests, sources, loc)
    if not any(graph['reach'].values()):
        raise ValueError('no test reaches any production source; missing rule-to-source edges')
    return graph, deps, hashes(source_hashes)


def crate_report(deps, tests, crates):
    """Count tests that read a crate, including through its sources filegroup."""
    crates, tests = sorted(crates), sorted(tests)
    closures = {test: closure(deps, test) for test in tests}
    markers = {crate: {crate, crate.split(':')[0] + ':sources'} for crate in crates}
    fanout = {crate: sum(bool(reach & markers[crate]) for reach in closures.values()) for crate in crates}
    edges = {(crate, dep) for crate in crates for dep in deps.get(crate, ()) if dep in crates}
    components = [members for members in graph_impact.components(crates, edges) if len(members) > 1]
    components.sort(key=lambda row: (-len(row), row))
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
            removed = sum(len(closures[test] - closure(modified, test)) for test in touched)
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
    changed = set(head['reach']) - set(base['reach'])
    deps = {}
    for consumer, dep in head['edges']:
        deps.setdefault(consumer, []).append(dep)
    for test in head['reach']:
        if direct_hash(head_hashes[test]) != direct_hash(base_hashes.get(test, '')):
            changed.add(test)
        package = test.split(':')[0].removeprefix('//')
        # Test sources are leaves; reach excludes them, so use the full edge closure.
        for label in closure(deps, test):
            file = path(label)
            if (file in changed_files and (Path(file).name.startswith('test_')
                                           or '/tests/' in file)
                    and file.startswith(package + '/')):
                changed.add(test)
    return changed


def policy_at(root, sha, file, default):
    result = subprocess.run(['git', 'show', f'{sha}:{file}'], cwd=root, capture_output=True, text=True)
    return json.loads(result.stdout) if not result.returncode else default

