"""History-free graph metrics and a baseline that only decreases.

The portable contract is Graph(tests={test: set(source)}, edges={(consumer, dep)},
loc={source: lines}). from_deps reads bazel-diff's label -> dependencies export.
No Git, Bazel, language parser, or CI history is needed by the core.
"""
from collections import Counter
from dataclasses import dataclass
import math
import statistics

EPSILON = 1e-9


@dataclass
class Graph:
    tests: dict
    edges: set
    loc: dict


def closure(deps, start):
    seen, pending = set(), [start]
    while pending:
        node = pending.pop()
        if node not in seen:
            seen.add(node)
            pending.extend(deps.get(node, ()))
    return seen


def from_deps(deps, tests, sources, loc=None):
    if not isinstance(deps, dict) or any(not isinstance(value, (list, set, tuple))
                                         for value in deps.values()):
        raise ValueError('deps.json must map labels to dependency lists')
    sources, tests = set(sources), set(tests)
    if not tests or tests - deps.keys():
        raise ValueError('test targets missing from dependency export: ' + ', '.join(sorted(tests - deps.keys())))
    if not sources:
        raise ValueError('no source labels in dependency export')
    nodes = set(deps) | {dep for values in deps.values() for dep in values}
    if sources - nodes:
        raise ValueError('source labels missing from dependency export')
    weights = {source: (loc or {}).get(source, 1) for source in sources}
    if any(not isinstance(value, int) or value < 0 for value in weights.values()):
        raise ValueError('LOC weights must be nonnegative integers')
    return Graph({test: closure(deps, test) & sources for test in sorted(tests)},
                 {(node, dep) for node, values in deps.items() for dep in values}, weights)


def cycles(edges):
    """Iterative Kosaraju; a deep build graph must not exhaust Python's stack."""
    forward, reverse = {}, {}
    for source, target in sorted(edges):
        forward.setdefault(source, []).append(target)
        reverse.setdefault(target, []).append(source)
        forward.setdefault(target, [])
        reverse.setdefault(source, [])
    visited, order = set(), []
    for start in sorted(forward):
        if start in visited:
            continue
        pending = [(start, False)]
        while pending:
            node, done = pending.pop()
            if done:
                order.append(node)
            elif node not in visited:
                visited.add(node)
                pending.append((node, True))
                pending.extend((dep, False) for dep in reversed(forward[node]) if dep not in visited)
    visited, result = set(), []
    for start in reversed(order):
        if start in visited:
            continue
        members, pending = set(), [start]
        while pending:
            node = pending.pop()
            if node not in visited:
                visited.add(node)
                members.add(node)
                pending.extend(reverse[node])
        if len(members) > 1 or (start, start) in edges:
            result.append(sorted(members))
    return sorted(result, key=lambda members: (-len(members), members))


def metrics(graph):
    fanout = Counter(source for reach in graph.tests.values() for source in reach)
    sources = set(graph.loc) | set(fanout)
    tests = len(graph.tests)
    if not tests or not sources:
        raise ValueError('graph must contain tests and sources')
    reaches = sorted(map(len, graph.tests.values()))
    components = cycles(graph.edges)
    total_loc = sum(graph.loc.get(source, 1) for source in sources)
    return {
        'tests': tests, 'sources': len(sources),
        'uniform': sum(fanout.values()) / len(sources) / tests,
        'loc_uniform': (sum(fanout[source] * graph.loc.get(source, 1) for source in sources)
                        / total_loc / tests if total_loc else 0),
        'fanout': {source: fanout[source] for source in sorted(sources)},
        'reach': {'median': statistics.median(reaches),
                  'p90': reaches[math.ceil(.9 * tests) - 1], 'max': reaches[-1]},
        'scc_max': len(components[0]) if components else 0, 'scc_members': components,
    }


def baseline(graph):
    measured = metrics(graph)
    return {'schema': 1, 'uniform': measured['uniform'], 'loc_uniform': measured['loc_uniform'],
            'scc_max': measured['scc_max'], 'reach_budget': math.ceil(measured['reach']['p90']),
            'tolerance': .001}


def write_baseline(graph, previous=None):
    current = baseline(graph)
    if previous is None:
        return current
    keys = ('uniform', 'scc_max', 'reach_budget')
    losses = [key for key in keys if current[key] > previous[key] + EPSILON]
    if losses:
        raise ValueError('baseline writer refuses losses: ' + ', '.join(losses))
    return {**previous, **{key: min(previous[key], current[key]) for key in keys}}


def check(base, head, changed_tests, policy, allowlist=None, previous_allowlist=None,
          previous_policy=None):
    allowlist = allowlist or {}
    if any(not isinstance(reason, str) or not reason.strip() for reason in allowlist.values()):
        raise ValueError('each allowlist entry needs a reason')
    old, new = metrics(base), metrics(head)
    added = sorted(head.edges - base.edges)
    errors, targets = [], set()
    if previous_allowlist is not None and any(previous_allowlist.get(key) != reason
                                              for key, reason in allowlist.items()):
        errors.append('allowlist may only shrink; new or rewritten exceptions are forbidden')
    if previous_policy:
        for key in ('uniform', 'scc_max', 'reach_budget', 'tolerance'):
            if policy[key] > previous_policy[key] + EPSILON:
                errors.append(f'baseline may not grow: {key} {previous_policy[key]} → {policy[key]}')
    changed = (set(head.tests) - set(base.tests)) | set(changed_tests)
    deps = {}
    for consumer, dependency in head.edges:
        deps.setdefault(consumer, []).append(dependency)
    for test in sorted(changed & head.tests.keys()):
        reach = head.tests[test]
        if len(reach) > policy['reach_budget'] and f'reach:{test}' not in allowlist:
            hub = max(reach, key=lambda source: (new['fanout'][source], source))
            errors.append(f'{test}: reach {len(reach)} > budget {policy["reach_budget"]}; hub {hub}')
            targets.add(test)
    for source, target in added:
        conftest = (source.endswith((':conftest.py', '/conftest.py'))
                    or any(dep.endswith((':conftest.py', '/conftest.py')) for dep in deps.get(source, ())))
        forbidden = (source in head.tests and target in head.tests
                     or conftest and bool(closure(deps, target) & head.loc.keys()))
        if forbidden and f'edge:{source} -> {target}' not in allowlist:
            errors.append(f'forbidden edge added: {source} -> {target}')
            targets.add(source)
    growth = sorted(((source, count - old['fanout'].get(source, 0))
                     for source, count in new['fanout'].items()
                     if count > old['fanout'].get(source, 0)), key=lambda item: (-item[1], item[0]))
    tolerance = policy.get('tolerance', EPSILON)
    for key, name in [('uniform', 'U')]:
        if new[key] > min(old[key], policy[key]) + tolerance and key not in allowlist:
            errors.append(f'{name} worsened: {old[key]:.6%} → {new[key]:.6%}; '
                          f'largest fanout growth: {growth[:5]}; added edges: {added[:20]}')
    old_components = list(map(set, old['scc_members']))
    for members in new['scc_members']:
        if (not any(set(members) <= previous for previous in old_components)
                and 'scc:' + ','.join(members) not in allowlist):
            errors.append('SCC grew or merged: ' + ', '.join(members)
                          + '; added edges: ' + str([edge for edge in added if edge[0] in members]))
    if (new['scc_max'] > policy['scc_max'] and any(
            len(members) > policy['scc_max'] and 'scc:' + ','.join(members) not in allowlist
            for members in new['scc_members'])):
        errors.append(f'SCC max {new["scc_max"]} > baseline {policy["scc_max"]}')
    # Reach budget is fixed at the recorded p90; legacy broad tests remain legal.
    for key in ('uniform', 'scc_max'):
        if new[key] < policy[key] - tolerance:
            errors.append(f'record gain for {key}: {policy[key]} → {new[key]}; run --write-baseline')
    if math.ceil(new['reach']['p90']) < policy['reach_budget']:
        errors.append(f'record gain for reach_budget: {policy["reach_budget"]} → '
                      f'{math.ceil(new["reach"]["p90"])}; run --write-baseline')
    if any('worsened' in error or 'SCC' in error for error in errors):
        consumers = {source for source, _ in added}
        targets.update(test for test in head.tests if closure(deps, test) & consumers)
    new_tests = sorted(set(head.tests) - set(base.tests))
    summary = (f'U {old["uniform"]:.3%} → {new["uniform"]:.3%}, '
               f'LOC U {old["loc_uniform"]:.3%} → {new["loc_uniform"]:.3%}, '
               f'SCC max {old["scc_max"]} → {new["scc_max"]}, '
               f'new tests {len(new_tests)} (reach {", ".join(str(len(head.tests[t])) for t in new_tests) or "none"}; '
               f'budget {policy["reach_budget"]}), new edges {len(added)}, '
               f'{len(errors)} violations')
    return {'summary': summary, 'errors': errors, 'base': old, 'head': new,
            'added_edges': [list(edge) for edge in added],
            'fanout_growth': [list(item) for item in growth], 'explain_targets': sorted(targets)}
