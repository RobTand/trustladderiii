"""P4 real-Python corpus generator. Run on BuildBuddy; --check guards drift."""
import ast
from contextvars import ContextVar
from hashlib import sha256
import json
from pathlib import Path
import random
import sys
from types import ModuleType, SimpleNamespace


def definitions(path, names, namespace):
    source = Path(path).read_text()
    tree = ast.parse(source)
    nodes = []
    for node in tree.body:
        assigned = {target.id for target in node.targets if isinstance(target, ast.Name)} if isinstance(node, ast.Assign) else set()
        if getattr(node, 'name', None) in names or assigned & names:
            nodes.append(node)
    exec(compile(ast.Module(body=nodes, type_ignores=[]), str(path), 'exec'), namespace)


def reference():
    root = Path(__file__).resolve().parents[4]
    workers = root / 'workers/src/tl3_workers'
    namespace = {'json': json, 'sha256': sha256, 'ContextVar': ContextVar, 'HumanMessage': SimpleNamespace}
    definitions(root / 'harness/src/tl3_harness/forcing_ownership.py', {'MAX_TOTAL_DELAY_SECONDS'}, namespace)
    definitions(root / 'harness/src/tl3_harness/model.py', {
        'MAX_DELAY_SECONDS', 'NATIVE_THREAD_ENVIRONMENT', 'RESERVED_ENVIRONMENT', 'RESERVED_PREFIXES'}, namespace)
    definitions(workers / 'evidence.py', {'forcing_capabilities', 'forcing_guidance'}, namespace)
    definitions(workers / 'forcing_rehearsal.py', {'MAX_REHEARSALS', 'REHEARSAL_SECONDS', 'GUIDANCE'}, namespace)
    definitions(workers / 'roles/planner.py', {'EXPLORATION_DECLARATION'}, namespace)
    namespace.update(
        files=lambda _: workers,
        schema=lambda name: json.loads((workers / 'schemas' / (name + '.json')).read_text()),
        run_prompt=ContextVar('reference_prompt', default=None),
        run_test_cpus=ContextVar('reference_cpus', default=1),
        ONLY_JSON='Reply with one JSON object only: no prose, no code fences.',
        ROLES={role: SimpleNamespace(name=role, prompt=role + '.md', reply_schema=role + '-reply')
               for role in ('ingest', 'audit', 'planner', 'maker', 'reviewer', 'classifier', 'writer', 'postmortem')})
    module = ModuleType('tl3_workers.forcing_rehearsal')
    module.GUIDANCE = namespace['GUIDANCE']
    sys.modules[module.__name__] = module
    exec(compile(Path(__file__).with_name('reference.py').read_text(), 'reference.py', 'exec'), namespace)
    return namespace


def corpus():
    oracle = reference()
    rows = []

    def add(kind, document, expected):
        rows.append({'kind': kind, 'document': json.dumps(document), 'expected': json.dumps(expected, sort_keys=True, ensure_ascii=True, separators=(',', ':'))})

    randomizer = random.Random(819)
    values = [None, True, False, '', '\"\\\b\f\n\r\t\x00\x1f/', 'é\u2028\u2029😀',
              0, -1, 2**64, -(2**100), 10**200, 0.0, -0.0, 1e-7, 1e-6, 1e15, 1e16,
              1e20, 1e23, 1.2345678901234567, 5e-324, 1.7976931348623157e308,
              {'z': {'é': [1, 1.0], 'a': '/'}, 'a': {'😀': True, '\ue000': False}},
              '\ud800', '\udfff']
    for _ in range(512):
        number = randomizer.uniform(-1, 1) * 10**randomizer.randint(-307, 307)
        values.extend([number, randomizer.randint(-(2**160), 2**160)])
    for value in values:
        add('serialize', value, {'text': json.dumps(value, sort_keys=True, ensure_ascii=False)})
    for role, spec in oracle['ROLES'].items():
        for cpus in (1, 7):
            for text in (None, '', 'Pinned é / 😀\n'):
                add('role', {'role': role, 'text': text, 'test_cpus': cpus},
                    {'text': oracle['role_prompt'](spec, text=text, test_cpus=cpus)})
    for role in ('ingest', 'audit', 'planner'):
        for inputs in ({}, {'base_sha': 'a' * 40}, {'repo': {'z': 1e16, 'a': ['é', 2**100]}},
                       {'base_sha': 'b' * 40, 'repo': {'nested': {'😀': '/', 'é': 1.0}}}):
            for model in ('claude-sonnet-5-5', 'gpt-6.1-sol'):
                request = {'role': role, 'inputs': inputs, 'trace': {'repo': 'owner/repo'},
                           'prompt': {'version': 3, 'text': 'Pinned é / 😀\n'}, 'test_cpus': 7,
                           'model': {'name': model}}
                add('cache', request, {'identity': oracle['prompt_cache_identity'](request)})
                message = oracle['initial_message'](request, {**inputs, 'specific': 'worker state'}).content
                add('prefix', request, {'prefix': message[0] if isinstance(message, list) else None})
    return rows


def main():
    path = Path(__file__).with_name('vectors.json')
    data = json.dumps(corpus(), ensure_ascii=True, indent=2) + '\n'
    if sys.argv[1:] == ['--check']:
        if path.read_text() != data:
            raise RuntimeError('P4 Python prompt identity corpus drifted')
        print('P4 Python corpus matches committed vectors')
    else:
        path.write_text(data)
        print(f'Generated {len(json.loads(data))} P4 vectors')


if __name__ == '__main__':
    main()
