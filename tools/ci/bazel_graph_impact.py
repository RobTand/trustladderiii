"""TL3's adapter for bazel-diff exports and missing source-edge evidence."""
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


def package(label):
    # Each Rust crate is one Bazel package, including its wrapper test targets.
    return canonical(label).split(':', 1)[0]


def package_max_reach(graph):
    maxima = {}
    for test, sources in graph['reach'].items():
        owner = graph['test_packages'][test]
        maxima[owner] = max(maxima.get(owner, 0), len(sources))
    return dict(sorted(maxima.items()))


def node_types(data):
    return {label: value.get('type') if isinstance(value, dict) else value.split('#', 1)[0]
            for label, value in hashes(data).items()}


def comparison_inputs(base, head):
    """Use base ownership ceilings and each revision's authoritative label types."""
    def classified(graph):
        kinds = graph['node_types']
        result = {'rule_to_rule': set(), 'rule_to_source': set()}
        for consumer, dependency in graph['edges']:
            if kinds.get(consumer) == 'Rule':
                if kinds.get(dependency) == 'Rule':
                    result['rule_to_rule'].add((consumer, dependency))
                elif kinds.get(dependency) == 'SourceFile':
                    result['rule_to_source'].add((consumer, dependency))
        return result
    old, new = classified(base), classified(head)
    old_packages = base.get('test_packages') or {test: package(test) for test in base['reach']}
    return {
        'test_packages': {test: package(test) for test in sorted(head['reach'])},
        'base_package_max_reach': package_max_reach({**base, 'test_packages': old_packages}),
        'edge_diff': {
            'added': {kind: [list(edge) for edge in sorted(new[kind] - old[kind])] for kind in new},
            'removed': {kind: [list(edge) for edge in sorted(old[kind] - new[kind])] for kind in old},
        },
    }


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
    measured['test_packages'] = {test: package(test) for test in sorted(tests)}
    measured['node_types'] = {node: 'SourceFile' if node in sources else 'Rule' for node in nodes}
    conftests = {node for node in deps if node.endswith((':conftest', ':conftest.py', '/conftest.py'))}
    measured['edge_groups']['conftest_source'] = sorted(
        f'{consumer} -> {dependency}' for consumer, dependency in edges
        if consumer in conftests and closure(deps, dependency) & sources)
    return measured


def baseline(graph):
    return ratchet(graph_impact.baseline_record(
        graph, tolerances={'u_pct': .1, 'loc_u_pct': .1, 'scc_max': 0}))


def ratchet(policy):
    """Persist numeric limits only; edge history comes from the actual base graph."""
    return {key: value for key, value in policy.items() if key != 'edge_groups'}


def check(base, head, changed, policy, allowlist=None, previous_allowlist=None, previous_policy=None,
          record_gains=False):
    head = {**head, **comparison_inputs(base, head)}
    errors = graph_impact.allowlist_errors(allowlist or {}, previous_allowlist if previous_allowlist is not None
                                          else allowlist or {})
    errors += graph_impact.baseline_errors(ratchet(policy), ratchet(previous_policy) if previous_policy else None)
    base_edges = set(map(tuple, base['edges']))
    # The core also checks absolute forbidden-edge exceptions. Supply those
    # transiently from base edges, including edges whose classification changed.
    # Never let a stored list either reject an old edge or excuse a new one.
    base_groups = {group: [edge for edge in rows if tuple(edge.split(' -> ', 1)) in base_edges]
                   for group, rows in head['edge_groups'].items()}
    comparison_policy = {**ratchet(policy), 'edge_groups': base_groups,
                         'figures': {key: value for key, value in policy['figures'].items() if key != 'scc_max'}}
    # SCC growth is checked component-by-component against base by compare().
    errors += graph_impact.compare({**base, 'edge_groups': base_groups}, head, comparison_policy,
                                   changed_tests=changed, allowlist=allowlist,
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
    broad = {test for test in targets if any(f'{test} reach ' in error for error in errors)}
    return {'summary': graph_impact.summary(base, head, policy['reach_budget']), 'errors': errors,
            'base': base, 'head': head, 'added_edges': [list(edge) for edge in sorted(added)],
            'edge_diff': head['edge_diff'], 'base_package_max_reach': head['base_package_max_reach'],
            'fanout_growth': [list(row) for row in growth],
            'explain_targets': sorted(targets, key=lambda test: (test not in broad, test))}


def write_baseline(graph, previous=None):
    if previous is None:
        return baseline(graph)
    errors = graph_impact.regressions({**graph, 'edge_groups': {}}, ratchet(previous))
    if errors:
        raise ValueError('baseline writer refuses losses: ' + '; '.join(errors))
    return ratchet(graph_impact.baseline_record(graph, ratchet(previous)))


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
    reaches, edges, weights = graph_impact.read_deps(deps_file, tests, sources, loc=loc)
    graph = graph_impact.measure(reaches, edges, weights)
    graph['test_packages'] = {test: package(test) for test in sorted(reaches)}
    graph['node_types'] = node_types(source_hashes)
    conftests = {node for node in deps if node.endswith((':conftest', ':conftest.py', '/conftest.py'))}
    graph['edge_groups']['conftest_source'] = sorted(
        f'{consumer} -> {dependency}' for consumer, dependency in edges
        if consumer in conftests and closure(deps, dependency) & sources)
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

