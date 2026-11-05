"""TL3's adapter for bazel-diff exports and missing source-edge evidence."""
import hashlib
import json
from pathlib import Path
import re
import subprocess
import tomllib

from child_env import child_env

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


def allowlist_errors(current, previous):
    errors = []
    for key, entry in sorted(current.items()):
        crate = key.startswith('crate:')
        reason = entry.get('reason') if crate and isinstance(entry, dict) else entry
        if crate and (not isinstance(entry, dict) or type(entry.get('sources')) is not int
                      or entry['sources'] < 0):
            errors.append(f'allowlist exception {key} needs a nonnegative integer sources count')
        if crate and (not isinstance(entry, dict) or type(entry.get('max_fanout')) is not int
                      or entry['max_fanout'] < 0):
            errors.append(f'allowlist exception {key} needs a nonnegative integer max_fanout')
        if not isinstance(reason, str) or not reason.strip():
            errors.append(f'allowlist exception {key} needs a reason')
        elif (crate or key not in previous) and not re.search(
                r'(?<!\w)#[1-9][0-9]*(?!\w)|'
                r'https://github\.com/RobTand/trustladderiii/issues/[1-9][0-9]*(?![\w/])', reason):
            errors.append(f'allowlist exception {key} needs a reason naming a program issue (#730, for example)')
        prior = previous.get(key)
        prior_reason = prior.get('reason') if crate and isinstance(prior, dict) else prior
        if key in previous and prior_reason != reason:
            errors.append(f'allowlist exception {key} reason cannot change')
    return errors


def bounded_crate_sources(base, head, allowlist, previous):
    """Validate exact head counts before granting package-scoped reach credits."""
    excused, errors = set(), []
    for key, entry in sorted(allowlist.items()):
        if (not key.startswith('crate:') or not isinstance(entry, dict)
                or type(entry.get('sources')) is not int or entry['sources'] < 0):
            continue
        name, limit = key.removeprefix('crate:'), entry['sources']
        owner = head.get('crate_packages', {}).get(name)
        if owner is None:
            errors.append(f'allowlist exception {key} has no Cargo package in the head graph')
            continue
        sources = {source for source in head['loc'] if package(source) == owner}
        if len(sources) != limit:
            errors.append(f'allowlist exception {key} has N={limit}, but the head graph has '
                          f'{len(sources)} sources; update N in the same change to match the source count')
            continue
        prior = previous.get(key)
        old_owner = base.get('crate_packages', {}).get(name)
        old_count = sum(package(source) == old_owner for source in base['loc'])
        if isinstance(prior, dict) and prior.get('sources') != limit and old_count == len(sources):
            errors.append(f'allowlist exception {key} cannot change N={limit} without a source-count change')
            continue
        excused.update(sources)
    return excused, errors


def bounded_crate_fanout(head, allowlist, crate_sources):
    """Exact absolute head fanout, independent of the comparison's base tests."""
    excused, errors = set(), []
    for key, entry in sorted(allowlist.items()):
        if not key.startswith('crate:'):
            continue
        owner = head.get('crate_packages', {}).get(key.removeprefix('crate:'))
        if owner is None:
            continue  # bounded_crate_sources reports unresolved packages.
        sources = {source for source in head['loc'] if package(source) == owner}
        measured = max((head['dependents'][source] for source in sources), default=0)
        limit = entry.get('max_fanout') if isinstance(entry, dict) else None
        if type(limit) is not int or limit < 0 or limit != measured:
            errors.append(f'allowlist exception {key} has max_fanout={limit!r}, but the head graph has '
                          f'max_fanout={measured}; update max_fanout in the same change to match the fanout')
        else:
            excused.update(sources & crate_sources)
    return excused, errors


def crate_reach_head(base, head, sources):
    # Subtract only gained labels after move normalization. Original identities
    # stay reachable, so a moved file cannot pay for another new dependency.
    return {**head, 'reach': {
        test: sorted(set(reached) - (sources - set(base['reach'].get(test, ()))))
        for test, reached in head['reach'].items()}}


def source_moves(base, head, edge_diff):
    """Pair unique byte-identical removed/added sources with source-edge evidence."""
    identities = []
    for graph, other, direction in ((base, head, 'removed'), (head, base, 'added')):
        by_identity = {}
        for source in sorted(graph['loc']):
            identity = graph.get('source_identities', {}).get(source)
            if identity:
                by_identity.setdefault(identity, []).append(source)
        candidates = (graph['loc'].keys() - other['loc'].keys()) & {
            dependency for _, dependency in edge_diff[direction]['rule_to_source']}
        identities.append({identity: sources[0] for identity, sources in by_identity.items()
                           if len(sources) == 1 and sources[0] in candidates})
    old, new = identities
    return dict(sorted((new[identity], old[identity]) for identity in old.keys() & new.keys()))


def moved_head(head, moves):
    # Base labels retain their package ownership for inheritance. Alias paths
    # attribute moved identities to actual added edges without changing SCCs.
    return {**head,
            'loc': {moves.get(source, source): weight for source, weight in head['loc'].items()},
            'reach': {test: sorted(moves.get(source, source) for source in sources)
                      for test, sources in head['reach'].items()},
            'dependents': {moves.get(source, source): count for source, count in head['dependents'].items()},
            'path_edges': [*head.get('path_edges', head['edges']), *map(list, moves.items())]}


def structural_reach_errors(base, head, allowlist):
    # The pinned core's aggregate structural-reach diagnostic ignores reach:
    # exceptions. Replace only that diagnostic; keep its edge attribution.
    inherited = graph_impact.inherited_sources(base, head)
    growth = {}
    for test in base['reach'].keys() & head['reach'].keys():
        effective = set(head['reach'][test]) - inherited.keys()
        gained = effective - set(base['reach'][test])
        # Deletions cannot pay for unmatched new sources, including ambiguous
        # copies. Only proven moves retain an original source identity.
        if (f'reach:{test}' not in allowlist
                and (len(effective) > len(base['reach'][test]) or gained - base['loc'].keys())):
            growth[test] = gained
    gained = set().union(*growth.values()) if growth else set()
    added = {tuple(edge) for rows in head['edge_diff']['added'].values() for edge in rows}
    causes = graph_impact._causing_edges(head, added, gained, growth) if gained else []
    if not causes:
        return []
    return ['existing test reach grew: ' + ', '.join(f'{test} +{len(growth[test])}' for test in sorted(growth))
            + '; added edges: ' + ', '.join(f'{a} -> {b}' for a, b in causes)]


def check(base, head, changed, policy, allowlist=None, previous_allowlist=None, previous_policy=None,
          record_gains=False):
    head = {**head, **comparison_inputs(base, head)}
    allowlist = allowlist or {}
    previous_allowlist = previous_allowlist if previous_allowlist is not None else allowlist
    errors = allowlist_errors(allowlist, previous_allowlist)
    crate_sources, crate_errors = bounded_crate_sources(base, head, allowlist, previous_allowlist)
    errors += crate_errors
    fanout_sources, fanout_errors = bounded_crate_fanout(head, allowlist, crate_sources)
    errors += fanout_errors
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
    moves = source_moves(base, head, head['edge_diff'])
    # Credits follow renamed identities only within their original package.
    credit_moves = {destination: origin for destination, origin in moves.items()
                    if package(destination) == package(origin)}
    crate_sources = {credit_moves.get(source, source) for source in crate_sources}
    fanout_sources = {credit_moves.get(source, source) for source in fanout_sources}
    comparison_head = moved_head(head, moves)
    compared = graph_impact.compare({**base, 'edge_groups': base_groups}, comparison_head, comparison_policy,
                                    changed_tests=changed, allowlist=allowlist, record_gains=record_gains)
    reach_head = crate_reach_head(base, comparison_head, crate_sources)
    if crate_sources:
        # Only per-test budgets and structural reach use the reach credits.
        # Fanout gets its own package-scoped credit below; SCC and edges stay full.
        budget_errors = graph_impact.compare(
            {**base, 'edge_groups': base_groups}, reach_head, comparison_policy,
            changed_tests=changed, allowlist=allowlist, record_gains=record_gains)
        compared = [error for error in compared if error.partition(' reach ')[0] not in head['reach']]
        compared += [error for error in budget_errors if error.partition(' reach ')[0] in head['reach']]
    if fanout_sources and any(error.startswith('existing source fanout grew: ') for error in compared):
        # Reuse the pinned core's consumer counting and edge attribution. Only
        # its fanout diagnostic uses this source universe; every other check
        # above sees the complete graph. Keep reach and identity paths intact.
        fanout_head = {**comparison_head, 'loc': {
            source: weight for source, weight in comparison_head['loc'].items()
            if source not in fanout_sources}}
        fanout_errors = graph_impact.compare(
            {**base, 'edge_groups': base_groups}, fanout_head, comparison_policy,
            changed_tests=changed, allowlist=allowlist, record_gains=record_gains)
        compared = [error for error in compared if not error.startswith('existing source fanout grew: ')]
        compared += [error for error in fanout_errors if error.startswith('existing source fanout grew: ')]
    errors += [error for error in compared if not error.startswith('existing test reach grew: ')]
    errors += structural_reach_errors(base, reach_head, allowlist)
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
    growth = sorted(((source, n - base['dependents'].get(moves.get(source, source), 0))
                     for source, n in head['dependents'].items()
                     if n > base['dependents'].get(moves.get(source, source), 0)),
                    key=lambda row: (-row[1], row[0]))
    broad = {test for test in targets if any(f'{test} reach ' in error for error in errors)}
    additions = {key: reason for key, reason in sorted(allowlist.items()) if key not in previous_allowlist}
    summary = graph_impact.summary(base, head, policy['reach_budget'])
    for key, reason in additions.items():
        summary += f'\nAdded allowlist entry {key}: {reason}'
    for key, entry in sorted(allowlist.items()):
        if key.startswith('crate:') and key in previous_allowlist and entry != previous_allowlist[key]:
            summary += f'\nChanged allowlist entry {key}: {previous_allowlist[key]} -> {entry}'
    for destination, origin in moves.items():
        summary += f'\nSource move {origin} -> {destination}'
    return {'summary': summary, 'errors': errors, 'allowlist_additions': additions, 'source_moves': moves,
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
    content = {label: (Path(root) / path(label)).read_bytes() for label in sources}
    loc = {label: len(data.splitlines()) for label, data in content.items()}
    reaches, edges, weights = graph_impact.read_deps(deps_file, tests, sources, loc=loc)
    graph = graph_impact.measure(reaches, edges, weights)
    graph['source_identities'] = {label: hashlib.sha256(data).hexdigest() for label, data in content.items()}
    graph['crate_packages'] = {}
    for owner in sorted({package(source) for source in sources}):
        manifest = Path(root) / path(owner + ':Cargo.toml')
        if manifest.is_file():
            name = tomllib.loads(manifest.read_text()).get('package', {}).get('name')
            if name:
                graph['crate_packages'][name] = owner
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
    result = subprocess.run(['git', 'show', f'{sha}:{file}'], cwd=root, capture_output=True, text=True, env=child_env())
    return json.loads(result.stdout) if not result.returncode else default

