"""Reproduce only the original #826 deterministic input population, not policy."""
import ast
import hashlib
from pathlib import Path


# Current generator sources are fixed so later fixture edits cannot silently
# change this historical population. Original source/population audit is in S1 evidence.
SOURCES = {
    'tracing_forms': '0cf8087c0501de6135c519e928ccf85ff4e5a311809770617c145f5dbd78ebcf',
    'tracing_forms3': 'b6a0c41f07776f02219b5b552cec29d314a461d27106ac94fde6e3ceba61ca8d',
}


def original_inputs():
    generated = {}
    for name, digest in SOURCES.items():
        path = Path(__file__).with_name(name + '.py')
        data = path.read_bytes()
        assert hashlib.sha256(data).hexdigest() == digest, 'historical input generator source changed'
        tree = ast.parse(data, filename=str(path))
        for definition in tree.body:
            if not isinstance(definition, ast.FunctionDef):
                continue
            if definition.name in {'r', 'public_r', 'public_content'}:
                # The original generators consumed this single RNG for both
                # public and confidential values, without a fake marker.
                definition.body = ast.parse("return ''.join(rng.choice(alpha) for _ in range(n))").body
                if definition.name == 'public_content':
                    definition.args = ast.parse('def r(n, alpha=ALNUM): pass').body[0].args
            elif definition.name == 'planted_secret':
                # Original descriptive fake values are generator inputs, never
                # committed URL credential strings or an encoded token corpus.
                definition.body = ast.parse("return '-'.join({1: ['postgres', 'password', '296'], 14: ['nested', 'password', '296'], 15: ['credential', '296']}[fixture_id])").body
        scope = {'__name__': name + '_original_inputs'}
        exec(compile(ast.fix_missing_locations(tree), str(path), 'exec'), scope)
        generated[name] = scope
    first, third = generated['tracing_forms'], generated['tracing_forms3']
    return ([text for _, text, *_ in first['FORMS'] + third['FORMS']]
            + [text for text, _ in first['PLANTED'] + first['PLANTED_LINES']])
