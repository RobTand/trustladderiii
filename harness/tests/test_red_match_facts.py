"""#836: raw RED facts from CPython/pytest and the real Python producer.

Run with --check for freshness, or --out PATH to regenerate on BuildBuddy.
Python owns rewriting, traceback/AST recovery and tokenization until S4c.
"""
import argparse
import ast
import json
from pathlib import Path
import sys
import tempfile

if __name__ == '__main__':
    sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'src'))

from _pytest.assertion.rewrite import rewrite_asserts
from tl3_harness import red_message

ROOT = Path(__file__).resolve().parents[1]
FIXTURE = ROOT.parent / 'crates/harness-core/tests/fixtures/red_match.json'


def text(value):
    return {'python_codepoints': list(map(ord, value))}


def tokens(value, *, needed=True):
    # CPython 3.14 tokenize cannot encode surrogates. capture never calls it
    # for these message/manual cases. None records unavailable facts, not a
    # fabricated token stream or a newly swallowed production exception.
    if not needed and any(0xD800 <= ord(char) <= 0xDFFF for char in value):
        return None
    parsed = red_message.source_tokens(value)
    if parsed is None:
        return None
    key, pairs = parsed
    return {'whitespace_removed_key': text(key),
            'tokens': [{'kind': kind, 'text': text(spelling)} for kind, spelling in pairs]}


def produce(source, declared, *, raw_override=None):
    with tempfile.TemporaryDirectory() as directory:
        path = Path(directory) / 'test_capture.py'
        path.write_text('def fail():\n    ' + source + '\n')
        tree = ast.parse(path.read_text())
        rewrite_asserts(tree, path.read_bytes(), str(path))
        namespace = {}
        exec(compile(tree, str(path), 'exec'), namespace)
        try:
            namespace['fail']()
        except AssertionError as error:
            # Exercise the helper's full domain with a real recovered traceback,
            # even when pytest normally appends an explanation to this message.
            if raw_override is not None:
                error.args = (raw_override,)
            assertion = red_message.rewritten_assertion(error)
            reason, match = red_message.capture(error, declared)
            facts = {'reason_raw': text(str(error)), 'assertion': None,
                     'declared': None if declared is None else {'text': text(declared),
                                                               'tokenization': tokens(declared, needed=(assertion is not None
                                                                                       and assertion[0].msg is None
                                                                                       and reason != declared))}}
            if assertion is not None:
                facts['assertion'] = {'has_explicit_msg': assertion[0].msg is not None,
                                      'segment': None if assertion[1] is None else tokens(assertion[1])}
            return facts, {'reason': text(reason), 'match': match}
        raise AssertionError('RED fact fixture must raise AssertionError')


def decode(value):
    if isinstance(value, dict) and set(value) == {'python_codepoints'}:
        return ''.join(map(chr, value['python_codepoints']))
    if isinstance(value, dict):
        return {key: decode(item) for key, item in value.items()}
    if isinstance(value, list):
        return [decode(item) for item in value]
    return value


def extra_cases():
    yield 'explicit-boundary-absent', 'assert False, "message"', 'first\n  second', 'first\n  second'
    yield 'multiple-boundaries', 'assert False, "first\\nassert embedded\\nassert again"', 'first', None
    yield 'manual-boundary', 'raise AssertionError("first\\n  second\\nassert manual")', 'first', None
    yield 'empty-explicit-message', 'assert False, ""', '', None
    yield 'embedded-indentation', 'assert False, "first\\n one\\n  second\\n   three\\n    third"', 'first\n one\nsecond\n three\n  third', None
    yield 'message-with-bad-tokens', 'raise AssertionError("assert (")', 'assert (', None
    yield 'manual-indentation', 'raise AssertionError("first\\n  second")', 'first\n  second', None
    yield 'surrogate-message', 'raise AssertionError("\\ud800")', '\ud800', None
    yield 'surrogate-versus-spelling', 'raise AssertionError("\\ud800")', r'\ud800', None
    yield 'surrogate-pair-versus-nonbmp', 'raise AssertionError("\\ud83d\\ude00")', '😀', None
    yield 'surrogate-pair-message', 'raise AssertionError("\\ud83d\\ude00")', '\ud83d\ude00', None
    yield 'nonbmp-versus-surrogate-pair', 'raise AssertionError("😀")', '\ud83d\ude00', None
    yield 'source-surrogate-literal', 'assert False and "\\ud800"', 'assert  False and "\\ud800"', None
    yield 'source-indentation-error', 'assert False', 'assert False\n    x\n  y', None
    yield 'long-message-disagreement', 'raise AssertionError(' + repr('x' * 1001 + 'A') + ')', 'x' * 1001 + 'B', None
    yield 'failing-location-only', 'if False:\n        assert 3 == 4\n    assert 1 == 2', 'assert 3 == 4', None
    yield 'explicit-no-declaration', 'assert False, "message"', None, None


def generate():
    original = decode(json.loads((ROOT / 'test-vectors/core/red-match.json').read_text()))
    cases = []
    for case in original['cases']:
        facts, expected = produce(**case['input'])
        # Original expectations are independent and remain authoritative.
        assert decode(expected['reason']) == case['expected']['reason'], case['id']
        assert expected['match'] == case['expected']['match'], case['id']
        cases.append({'id': case['id'], 'origin': 'core', 'facts': facts, 'expected': expected})
    assert len(cases) == 17
    for name, source, declared, raw_override in extra_cases():
        facts, expected = produce(source, declared, raw_override=raw_override)
        cases.append({'id': name, 'origin': 'edge', 'facts': facts, 'expected': expected})
    return {'kind': 'red-match-facts', 'cases': cases}


def rendered():
    return json.dumps(generate(), indent=2, ensure_ascii=True) + '\n'


def test_fixture_freshness():
    assert FIXTURE.read_text() == rendered(), 'regenerate test_red_match_facts.py --out on BuildBuddy'


def test_producer_contract():
    cases = {case['id']: case for case in generate()['cases']}
    continuation = cases['continuation']['facts']
    segment = continuation['assertion']['segment']
    declaration = continuation['declared']['tokenization']
    assert segment['tokens'] == declaration['tokens']
    assert segment['whitespace_removed_key'] != declaration['whitespace_removed_key']
    assert cases['source']['expected']['match'] == 'message'
    assert cases['manual-source']['facts']['assertion'] is None
    assert cases['message-with-bad-tokens']['facts']['declared']['tokenization'] is None
    assert cases['message-with-bad-tokens']['expected']['match'] == 'message'
    assert cases['surrogate-versus-spelling']['expected']['match'] == 'mismatch'
    assert cases['surrogate-pair-versus-nonbmp']['expected']['match'] == 'mismatch'
    assert cases['surrogate-pair-message']['expected']['match'] == 'message'
    assert cases['failing-location-only']['expected']['match'] == 'mismatch'
    assert cases['explicit-boundary-absent']['expected']['reason'] == text('first\n  second')
    assert cases['multiple-boundaries']['expected']['reason'] == text('first')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument('--check', action='store_true')
    mode.add_argument('--out', type=Path)
    args = parser.parse_args()
    if args.check:
        test_fixture_freshness()
        test_producer_contract()
        print('red-match facts: 17 core + 17 edge cases fresh; producer contract passed')
    else:
        args.out.write_text(rendered())
