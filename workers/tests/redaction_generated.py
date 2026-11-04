"""#753: corpus-seeded full-redactor probes and a witness for every policy class."""
import json
import random
import re
from re import _parser
import string

from redaction_branches import FAKE, branch_inputs, original_inputs

SHARDS = 8
# Every ASCII delimiter, all whitespace, Python's exceptional ASCII folds, and Kelvin sign.
DELIMITERS = string.punctuation + string.whitespace + 'İıKKſ'


def expressions(policy):
    for section in ['regex', 'helpers', 'patterns', 'checked']:
        entries = policy[section]
        for name, value in (entries.items() if isinstance(entries, dict) else enumerate(entries)):
            yield f'{section}:{name}', value[0] if section == 'checked' else value


def class_atoms(pattern):
    """Tokenize classes independently of the Rust parser, preserving source occurrences."""
    tokens = _parser.Tokenizer(pattern)
    while (token := tokens.get()) is not None:
        if token == '[':
            atom = '['
            if tokens.match('^'):
                atom += '^'
            if tokens.match(']'):
                atom += ']'
            while (token := tokens.get()) is not None:
                atom += token
                if token == ']':
                    yield atom
                    break
            else:
                raise AssertionError('unfinished class')
        elif token in [r'\w', r'\W', r'\d', r'\D', r'\s', r'\S']:
            yield '[' + token + ']'


def alphabet(tree):
    """Literal/range witnesses from Python's parsed expression, not a generic alphabet."""
    result = set()
    for op, arg in tree:
        if op in [_parser.LITERAL, _parser.NOT_LITERAL]:
            result.add(chr(arg))
        elif op == _parser.RANGE:
            lo, hi = arg
            result.update(chr(c) for c in [lo, hi, (lo + hi) // 2])
            result.update(chr(c) for c in [lo - 1, hi + 1] if 0 <= c <= 0x10ffff
                          and not 0xd800 <= c <= 0xdfff)
        elif op == _parser.IN:
            result.update(alphabet(arg))
        elif op == _parser.SUBPATTERN:
            result.update(alphabet(arg[-1]))
        elif op in [_parser.MAX_REPEAT, _parser.MIN_REPEAT, _parser.POSSESSIVE_REPEAT]:
            result.update(alphabet(arg[-1]))
        elif op == _parser.BRANCH:
            for branch in arg[1]:
                result.update(alphabet(branch))
        elif op in [_parser.ASSERT, _parser.ASSERT_NOT]:
            result.update(alphabet(arg[1]))
        elif op == _parser.ATOMIC_GROUP:
            result.update(alphabet(arg))
    return result


def class_vectors(atom):
    candidates = set(map(chr, range(128))) | set(DELIMITERS)
    candidates.update(alphabet(_parser.parse(atom, 0)))
    result = []
    # Exercise every class under both flags, covering every actual scoped flag state.
    for flags in [0, re.I]:
        regex = re.compile(atom, flags)
        positive = [c for c in sorted(candidates) if regex.fullmatch(c)]
        negative = [c for c in sorted(candidates) if not regex.fullmatch(c)]
        assert positive and negative, f'class lacks membership witnesses: {atom!r}'
        # Include an actual adjacent membership boundary, rather than an arbitrary negative.
        boundary = next(((a, b) for a in sorted(candidates)
                         for b in [chr(ord(a) + 1)] if ord(a) < 0x10ffff
                         and bool(regex.fullmatch(a)) != bool(regex.fullmatch(b))), None)
        assert boundary is not None, f'class lacks an adjacent boundary: {atom!r}'
        probes = sorted(candidates | set(boundary))
        result.append({'pattern': ('(?i)' if flags else '') + atom,
                       'texts': probes, 'expected': [bool(regex.fullmatch(c)) for c in probes],
                       'inside': positive[0], 'outside': negative[0], 'boundary': boundary})
    return result


def generate(policy, corpus):
    rng = random.Random(753)
    seeds = list(dict.fromkeys(original_inputs() + branch_inputs() + corpus))
    shards = [{'classes': [], 'inputs': []} for _ in range(SHARDS)]
    total_classes = 0
    for index, (name, pattern) in enumerate(expressions(policy)):
        shard = shards[index % SHARDS]
        letters = set(DELIMITERS) | alphabet(_parser.parse(pattern, 0))
        for occurrence, atom in enumerate(class_atoms(pattern)):
            records = class_vectors(atom)
            for record in records:
                record['label'] = f'{name}/class:{occurrence}'
                shard['classes'].append(record)
                letters.update([record['inside'], record['outside'], *record['boundary']])
            total_classes += 1
        regex = re.compile(pattern)
        examples = [(seed, match) for seed in seeds if len(seed) <= 256
                    if (match := regex.search(seed)) is not None and match.end() > match.start()][:2]
        assert examples, f'no planted seed for {name}'
        for seed, match in examples:
            # Inject each expression's own alphabet, delimiters and folding letters into
            # planted secrets and into the expression's matched region and its boundaries.
            for char in sorted(letters):
                payload = FAKE[:4] + char + FAKE[4:]
                offset = rng.randrange(match.start(), match.end() + 1)
                shard['inputs'].extend([
                    seed[:offset] + char + seed[offset:],
                    seed.replace(FAKE, payload) if FAKE in seed
                    else seed[:match.start()] + payload + seed[match.end():],
                    char + seed + char,
                ])
    assert total_classes > 200, 'policy class coverage unexpectedly shrank'
    for index, seed in enumerate(seeds):
        shards[index % SHARDS]['inputs'].append(seed)
    for shard in shards:
        shard['inputs'] = list(dict.fromkeys(shard['inputs']))
    # Always include the exact reviewer's probe independently of corpus sampling.
    shards[0]['inputs'][:0] = ['Basic zE9p-R4cV7x', 'Basıc zE9p-R4cV7x',
                              'Basic zE9pR4cV7x-tail', 'Basİc zE9pR4cV7x-tail']
    return {'shards': shards, 'class_occurrences': total_classes}


def main():
    import sys
    from pathlib import Path
    policy = json.loads(Path(sys.argv[1]).read_text())
    corpus = [text for path in sys.argv[2:] for text in json.loads(Path(path).read_text())]
    print(json.dumps(generate(policy, corpus)))


if __name__ == '__main__':
    main()
