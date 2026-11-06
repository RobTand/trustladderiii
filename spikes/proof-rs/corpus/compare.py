"""Compare by call id and primary R1 rule, retaining all counterexamples."""
import ast
from collections import Counter, defaultdict
import json
from pathlib import Path
import statistics
import sys

corpus, out = map(Path, sys.argv[1:])

def rows(path):
    return [json.loads(line) for line in path.read_text().splitlines()]

sys.path.insert(0, str(Path('harness/src').resolve()))
# The runner cwd is spikes/proof-rs.
sys.path.insert(0, str(Path('../../harness/src').resolve()))
from tl3_harness import proof

expected = rows(corpus / 'calls.jsonl')
report = {}
for mode in ('current', 'reduced'):
    actual = {row['id']: row for row in rows(out / (mode + '.log'))}
    counts = Counter()
    rules = defaultdict(Counter)
    differences = []
    pairs = []
    for row in expected:
        python = row['python']
        rust = actual.get(row['id'])
        if rust is None:
            counts['missing'] += 1
            continue
        if 'accepted' not in python:
            counts['python_error'] += 1
            continue
        rule = 'accepted' if python['accepted'] else python['rule']
        group = rules[rule]
        counts['calls'] += 1
        group['calls'] += 1
        verdict = python['accepted'] == rust['accepted']
        first_rule = verdict and (python['accepted'] or python['rule'] == rust['rule'])
        receipt = bool(python['accepted'] and rust['accepted'] and python['receipt'] == rust['receipt'])
        stage = bool(python['accepted'] and rust['accepted'] and python['staged_sha256'] == rust['staged_sha256'])
        for key, flag in [('verdict', verdict), ('first_rule', first_rule), ('receipt', receipt), ('stage', stage)]:
            counts[key] += flag
            group[key] += flag
        if not first_rule or python['accepted'] and (not receipt or not stage):
            entry = {'id': row['id'], 'node': row['node'], 'python': rule,
                     'rust': 'accepted' if rust['accepted'] else rust['rule']}
            if python['accepted'] and rust['accepted']:
                entry['receipt_differences'] = [key for key in python['receipt'] if python['receipt'][key] != rust['receipt'].get(key)]
                entry['file_differences'] = [key for key in set(python['staged']) | set(rust['staged']) if python['staged'].get(key) != rust['staged'].get(key)]
            differences.append(entry)
        if row.get('python_ns') and rust.get('rust_ns'):
            pairs.append((row['python_ns'], rust['rust_ns']))
    def timings(index):
        values = sorted(pair[index] / 1000 for pair in pairs)
        return {'n': len(values), 'median_us': statistics.median(values), 'p95_us': values[int(len(values) * .95)], 'total_us': sum(values)}
    report[mode] = {'counts': counts, 'rules': dict(rules), 'differences': differences,
                    'latency': {'python': timings(0), 'rust': timings(1)},
                    'warning': 'Python on BuildBuddy includes filesystem staging. Rust on PB uses in-memory file maps. These are descriptive latencies, not a controlled speedup.'}
source_expected = {row['sha256']: row for row in rows(corpus / 'sources.jsonl')}
counts = Counter()
normalizations = []
semantics = []
literals = []
for row in rows(out / 'sources.log'):
    py = source_expected[row['sha256']]
    counts['sources'] += 1
    counts['parse_agrees'] += row['accepted'] == ('error' not in py)
    if row['accepted'] and 'error' not in py:
        for key, function in [('binding_names', proof.binding_names), ('global_names', proof.global_names), ('written_bindings', proof.written_bindings)]:
            try:
                value = function(py['source'], 'probe') if key != 'written_bindings' else function(py['source'], filename='probe')
                if key == 'written_bindings':
                    value = {'::'.join(item) for item in value}
                value = sorted(value)
            except (SyntaxError, ValueError):
                value = None
            if value != row.get(key):
                semantics.append({'sha256': row['sha256'], 'name': py['name'], 'kind': key, 'source': py['source'], 'python': value, 'rust': row.get(key)})
            else:
                counts[key + '_equal'] += 1
        tree = ast.parse(py['source'])
        for node in ast.walk(tree):
            if isinstance(node, (ast.Assign, ast.AnnAssign)) and node.value is not None:
                source = ast.get_source_segment(py['source'], node.value)
                try:
                    value = ast.literal_eval(node.value)
                    try:
                        hash(value)
                        expected_literal = True
                    except TypeError:
                        expected_literal = False
                except (SyntaxError, TypeError, ValueError):
                    expected_literal = None
                if source in row['literals'] and row['literals'][source] != expected_literal:
                    literals.append({'sha256': row['sha256'], 'name': py['name'], 'source': source, 'python': expected_literal, 'rust': row['literals'][source]})
        counts['comparable_roundtrip'] += row['comparable_roundtrip']
        counts['unparse_equal'] += py['unparse'] == row['generated']
        if py['unparse'] != row['generated']:
            normalizations.append({'sha256': row['sha256'], 'name': py['name'], 'source': py['source'],
                                   'python': py['unparse'], 'ruff': row['generated']})
report['parser'] = {'counts': counts, 'normalization_differences': normalizations, 'semantic_differences': semantics, 'literal_differences': literals}
metadata = json.loads((out / 'metadata.log').read_text())
report['dependencies'] = {'packages': len(metadata['packages']),
    'licenses': dict(Counter(package.get('license') or '<unspecified>' for package in metadata['packages'])),
    'inventory': [{'name': package['name'], 'version': package['version'], 'license': package.get('license'),
                   'source': package.get('source')} for package in metadata['packages']]}
(out / 'comparison.json').write_text(json.dumps(report, indent=2) + '\n')
print(json.dumps({mode: {key: value for key, value in data.items() if key in ('counts', 'latency', 'licenses', 'packages')} for mode, data in report.items()}, indent=2))
