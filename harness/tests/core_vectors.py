"""Generate #749 parity data by calling the current Python harness, never a mirror."""
import argparse
import ast
from copy import deepcopy
from dataclasses import asdict
from itertools import product
import json
from pathlib import Path
import sys
import tempfile
from types import SimpleNamespace

if __name__ == '__main__':
    sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'src'))

from _pytest.assertion.rewrite import rewrite_asserts
from tl3_harness import limits, model, node_ids, red_message, runner, test_tool
from tl3_harness.selector_identity import selection_matches, selector_identity

ROOT = Path(__file__).resolve().parents[1] / 'test-vectors'
KINDS = ('node-id', 'selector-identity', 'summary-bound', 'red-match', 'limits', 'judge', 'phase-fold')
NODE = 'tests/test_value.py::test_value'


def wire(value):
    """JSON strings cannot carry lone surrogates in serde; retain their Python code points."""
    if isinstance(value, str):
        if any(0xD800 <= ord(char) <= 0xDFFF for char in value):
            return {'python_codepoints': list(map(ord, value))}
        return value
    if isinstance(value, dict):
        return {key: wire(item) for key, item in value.items()}
    if isinstance(value, (list, tuple)):
        return [wire(item) for item in value]
    return value


def outcome(function, *args):
    try:
        result = function(*args)
        return {'value': result}
    except ValueError as error:
        return {'error': str(error)}


def cases(kind):
    if kind == 'node-id':
        legacy = json.loads((ROOT / 'pytest-node-ids.json').read_text())
        nodes = [item['node_id'] for item in legacy]
        nodes += ['tests/test_é.py::test_𝛂', 'tests/test_x.py::test_x[\u0085]',
                  'tests/test_x.py::test_x[\x7f]', 'tests/[x].py::test_x',
                  'tests/test_x.py::test_x[a::b]', 'tests/test_x.py', '.git/test_x.py::test_x']
        for index, node in enumerate(dict.fromkeys(nodes)):
            yield f'grammar-{index:02}', {'op': 'grammar', 'node_id': node}
        for index, item in enumerate(item for item in legacy if 'collected' in item):
            yield f'selects-{index}', {'op': 'selects', 'requested': item['node_id'], 'collected': item['collected']}
        for name, tests in [('tuple-shape', None), ('not-strings', [1]), ('too-many', [NODE] * 21),
                            ('duplicates', [NODE, NODE]), ('empty', [])]:
            yield name, {'op': 'always-tests', 'tests': tests}
    elif kind == 'selector-identity':
        selections = [[], [NODE], [NODE, NODE], [NODE, 'test_é.py::test_😀'],
                      ['test_é.py::test_😀', NODE], ['é😀' * 80] * 4, ['x\ud800y'],
                      ['quote"\\\n\t', 'e\u0301', 'é']]
        for index, selectors in enumerate(selections):
            yield f'identity-{index}', {'selectors': selectors, 'comparison': 'identity'}
        for comparison in ('full-list', 'prefix-only', 'float-count', 'bool-count', 'wrong-tail', 'wrong-digest'):
            yield comparison, {'selectors': [NODE] * 4, 'comparison': comparison}
    elif kind == 'summary-bound':
        for name, message in [('ascii', 'x' * 1001), ('unicode', 'é😀' * 501),
                              ('surrogate', '\ud800' * 1001), ('at-limit', '😀' * 1000)]:
            yield name, {'op': 'summary', 'entries': [{'test': NODE, 'outcome': 'failed', 'message': message}]}
        yield 'describe-splitlines-strip', {'op': 'describe', 'message': '\u2003\x85\u00a0`first`\u2003\u2028second\r\nthird'}
        yield 'describe-unicode-limit', {'op': 'describe', 'message': '😀' * 201}
        for name, message in [('byte-limit-ascii', 'a' * 100), ('byte-limit-unicode', '😀' * 100),
                              ('byte-limit-surrogate', '\udfff' * 100)]:
            yield name, {'op': 'summary', 'limit': 700,
                         'entries': [{'test': NODE, 'outcome': 'failed', 'message': message}]}
        yield 'notices-once', {'op': 'summary', 'limit': 400,
                              'entries': [{'test': str(i), 'outcome': 'passed', 'message': ''} for i in range(5)]}
        yield 'identity-compaction', {'op': 'summary', 'limit': 1000, 'selectors': [NODE + 'x' * 2000],
                                     'entries': [{'test': NODE + 'x' * 2000, 'outcome': 'passed', 'message': ''}]}
        yield 'identity-cannot-fit', {'op': 'summary', 'limit': 500, 'immutable': 'x' * 1000, 'entries': []}
        group = {'status': 'completed', 'tests': [{'test': str(i), 'outcome': o, 'message': 'failure' * 80}
                 for i, o in enumerate(['failed', 'passed', 'error', 'passed'])],
                 'totals': {'passed': 2, 'failed': 1, 'error': 1, 'skipped': 0}, 'note': '',
                 'proof': {'mode': 'pytest-outcome', 'source_base_tree_sha256': 'a' * 64}}
        yield 'nested-groups-and-base', {'op': 'envelope', 'limit': 1800,
                                        'result': {**deepcopy(group), 'groups': [deepcopy(group)], 'base': deepcopy(group),
                                                   'receipts': ['receipt'], 'action_key': 'b' * 64}}
        yield 'run-failures', {'op': 'envelope', 'limit': 400,
                              'result': {'tests': [], 'totals': {}, 'runs': [{'failed': ['x' * 800, 'y' * 800]}]}}
    elif kind == 'red-match':
        statement = 'assert 1 == 2'
        for name, source, declared in [
            ('message', 'assert 1 == 2, "too small"', 'too small'),
            ('message-prefix', 'assert 1 == 2, "too small"', 'too'),
            ('explicit-source-refused', 'assert 1 == 2, "too small"', statement),
            ('source', statement, statement), ('source-spaces', statement, 'assert  1  ==  2'),
            ('source-newlines', 'assert (1\n        == 2)', 'assert (1 == 2)'),
            ('literal-whitespace', 'assert 1 == 2 and "a b"', 'assert 1 == 2 and "a  b"'),
            ('literal-quotes', 'assert 1 == 2 and "a"', "assert 1 == 2 and 'a'"),
            ('continuation', statement, 'assert 1 \\\n == 2'),
            ('comment', statement, statement + ' # comment'),
            ('manual-source', 'raise AssertionError("other\\nassert 1 == 2")', statement),
            ('manual-message', 'raise AssertionError("too small")', 'too small'),
            ('no-declaration', statement, None), ('bad-tokens', statement, 'assert (1 == 2'),
            ('unicode-whitespace', statement, 'assert\u00a01 == 2'),
        ]:
            yield name, {'source': source, 'declared': declared}
    elif kind == 'limits':
        for repeat, parallel in product([0, 1, 10, 11, 1.0, True], [0, 1, 2, 4, 5, 2.0, False]):
            yield f'options-{repeat!r}-{parallel!r}', {'op': 'options', 'repeat': repeat, 'parallel': parallel}
        for name, tests in [('empty', []), ('deduplicated', [NODE] * 3), ('unicode', ['é😀']),
                            ('surrogate', ['x\ud800']), ('escaped', ['"\\\n'])]:
            yield 'plan-' + name, {'op': 'plan-size', 'tests': tests}
        # Encoded collection includes spaces, sorted keys and a final newline. The largest phase adds 8192 bytes.
        for char, count in [('x', limits.MAX_REPORT_LINE_BYTES - 8194),
                            ('😀', (limits.MAX_REPORT_LINE_BYTES - 8194) // 4)]:
            for delta in (0, 1):
                yield f'phase-boundary-{ord(char)}-{delta}', {'op': 'plan-size-repeat', 'char': char, 'count': count + delta}
        for count in (0, 1, 500, 2000):
            yield f'check-plan-{count}', {'op': 'check-plan', 'count': count}
        for value in [0, 1, 1.0, True, 5, 5.01]:
            yield f'delay-{value!r}', {'op': 'forcing', 'forcing': {'symptom': 'delay', 'patches': [
                {'target': 'pkg.func', 'delay_seconds': value}]}}
        for target in ['pkg.fé', 'pkg.f١', 'pkg.١', 'pkg.fn']:
            yield 'target-' + target, {'op': 'forcing', 'forcing': {'symptom': 'delay', 'patches': [
                {'target': target, 'delay_seconds': 1}]}}
    elif kind == 'judge':
        for check_kind, before, after in product(model.Kind, model.Outcome, model.Outcome):
            yield f'{check_kind}-{before}-{after}', {'check_kind': check_kind, 'base': before, 'head': after,
                                                   'proof': 'message'}
        for before, after, forced in product(model.Outcome, repeat=3):
            yield f'forced-{before}-{after}-{forced}', {'check_kind': 'demonstrating', 'base': before,
                                                       'head': after, 'forced': forced, 'proof': 'message'}
        for check_kind, proof in product(['demonstrating', 'reproducing'],
                                        ['absent', 'missing-failure', 'mismatch', 'source-assertion',
                                         'wrong-declaration', 'setup']):
            yield f'{check_kind}-{proof}', {'check_kind': check_kind, 'base': 'assertion', 'head': 'passed', 'proof': proof}
    elif kind == 'phase-fold':
        states = [('passed', False), ('skipped', False), ('failed', False), ('failed', True)]
        for index, phases in enumerate(product(states, repeat=3)):
            yield f'phases-{index:02}', {'phases': [[when, state, '', False, assertion]
                                        for when, (state, assertion) in zip(('setup', 'call', 'teardown'), phases)]}
        yield 'truncated-unicode', {'phases': [['call', 'failed', '😀' * 1001, True, True],
                                              ['teardown', 'passed', '', False, False]]}
        yield 'worker-crash', {'phases': [['???', 'failed', 'worker crashed', False, False]], 'parallel': True,
                               'when': '???'}


def evaluate(kind, data):
    if kind == 'node-id':
        if data['op'] == 'always-tests':
            return outcome(node_ids.validate_always_tests, data['tests'])
        if data['op'] == 'selects':
            return [node_ids.selects(data['requested'], node) for node in data['collected']]
        node = data['node_id']
        return {'exact': accepts_check(node), 'always': outcome(node_ids.validate_always_tests, [node]),
                'selected': accepts_check(node, selected=True)}
    if kind == 'selector-identity':
        selectors = data['selectors']
        identity = selector_identity(selectors)
        comparison = data['comparison']
        candidate = deepcopy(identity)
        if comparison == 'full-list':
            candidate = selectors
        elif comparison == 'prefix-only':
            candidate = identity['prefix']
        elif comparison == 'float-count':
            candidate['count'] = float(candidate['count'])
        elif comparison == 'bool-count':
            candidate['count'] = True
        elif comparison == 'wrong-tail':
            candidate = selectors[:-1] + ['other.py::test_other']
        elif comparison == 'wrong-digest':
            candidate['sha256'] = '0' * 64
        return {'identity': identity, 'candidate': candidate, 'matches': selection_matches(candidate, selectors)}
    if kind == 'summary-bound':
        if data['op'] == 'describe':
            return runner.describe(ValueError(data['message']))
        if data['op'] == 'summary':
            summary = test_tool.Summary()
            for entry in data['entries']:
                summary.add(**entry)
            result = summary.result
            result.update({key: data[key] for key in ('selectors', 'immutable') if key in data})
        else:
            result = deepcopy(data['result'])
        result = test_tool.bounded(result, **({'limit': data['limit']} if 'limit' in data else {}))
        return {'result': result, 'json_bytes': len(json.dumps(result).encode())}
    if kind == 'red-match':
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'test_capture.py'
            path.write_text('def fail():\n    ' + data['source'] + '\n')
            tree = ast.parse(path.read_text())
            rewrite_asserts(tree, path.read_bytes(), str(path))
            namespace = {}
            exec(compile(tree, str(path), 'exec'), namespace)
            try:
                namespace['fail']()
            except AssertionError as error:
                reason, match = red_message.capture(error, data['declared'])
                return {'reason': reason, 'match': match, 'tokens': red_message.source_tokens(data['declared'])
                        if data['declared'] is not None else None}
            raise AssertionError('RED fixture must raise AssertionError')
    if kind == 'limits':
        op = data['op']
        if op == 'options':
            return outcome(test_tool.options, data['repeat'], data['parallel'])
        if op == 'check-plan':
            check = SimpleNamespace(name='vector', kind=model.Kind.REGRESSION,
                                    tests=[f'tests/test_x.py::test_{i}' for i in range(data['count'])])
            return outcome(limits.validate_check_plan, [check])
        if op == 'forcing':
            return outcome(lambda value: model.Forcing.parse(value).as_json(), data['forcing'])
        tests = data['tests'] if op == 'plan-size' else [data['char'] * data['count']]
        encoded = json.dumps({'event': 'collection', 'tests': tuple(dict.fromkeys(tests))},
                             sort_keys=True, ensure_ascii=False).encode(errors='backslashreplace') + b'\n'
        return {'validation': outcome(limits.validate_plan_size, tests), 'collection_bytes': len(encoded)}
    if kind == 'judge':
        proof_mode = data['proof']
        proof = (model.Proof((), (), 'expected')
                 if proof_mode != 'absent' and data['check_kind'] in ('demonstrating', 'reproducing') else None)
        forcing = model.Forcing('forced', (('REPRODUCE', '1'),)) if 'forced' in data else None
        check = model.Check('vector', model.Kind(data['check_kind']), (NODE,), forcing=forcing, proof=proof)
        notes = model.FailureNotes('other' if proof_mode == 'wrong-declaration' else 'expected')
        if proof_mode != 'missing-failure':
            notes.details[NODE] = model.Failure('setup' if proof_mode == 'setup' else 'call', 'AssertionError', '',
                                              red_message_match=proof_mode if proof_mode in ('message', 'source-assertion')
                                              else 'mismatch')
        result = model.judge(check, {NODE: model.Outcome(data['base'])}, {NODE: model.Outcome(data['head'])},
                             base_notes=notes, forced={NODE: model.Outcome(data['forced'])} if forcing else None)
        return asdict(result)
    if kind == 'phase-fold':
        reporter = runner.Reporter([NODE], -1, summary=True, parallel=data.get('parallel', False))
        reporter.phases[NODE] = deepcopy(data['phases'])
        records = []
        reporter.emit = lambda **record: records.append(record)
        # Feed the already-captured phase facts to the real plugin's fold, without running pytest children.
        reporter.pytest_runtest_logreport(SimpleNamespace(when=data.get('when', 'teardown'), nodeid=NODE,
                                                         longrepr=None, failed=False, outcome='passed'))
        return records
    raise AssertionError(f'unknown kind: {kind}')


def accepts_check(node, selected=False):
    try:
        model.Check(model.SELECTED_CHECK if selected else 'vector',
                    model.Kind.REGRESSION if selected else model.Kind.REPRODUCING, (node,))
        return True
    except ValueError:
        return False


def vectors(kind):
    return {'kind': kind, 'cases': [{'id': name, 'input': wire(data), 'expected': wire(evaluate(kind, data))}
                                   for name, data in cases(kind)]}


def check(kind, root=ROOT / 'core'):
    produced = vectors(kind)
    path = root / f'{kind}.json'
    committed = json.loads(path.read_text()) if path.exists() else {'cases': []}
    assert committed.get('kind') == kind, f'{kind}: missing or wrong kind in {path}'
    expected = {case['id']: case for case in committed['cases']}
    actual = {case['id']: case for case in produced['cases']}
    assert expected.keys() == actual.keys(), f'{kind}: case inventory changed; regenerate core vectors'
    for name, case in actual.items():
        assert expected[name] == case, f'{kind}/{name}: differs from Python; regenerate core vectors'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--check', action='store_true', help='fail on drift, naming the kind and case')
    parser.add_argument('--output', type=Path, default=ROOT / 'core')
    args = parser.parse_args()
    for kind in KINDS:
        if args.check:
            check(kind, args.output)
        else:
            args.output.mkdir(parents=True, exist_ok=True)
            (args.output / f'{kind}.json').write_text(json.dumps(vectors(kind), indent=1, sort_keys=True) + '\n')
        print(kind, len(list(cases(kind))), 'checked' if args.check else 'generated')


if __name__ == '__main__':
    main()
