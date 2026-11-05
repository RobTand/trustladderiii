"""Both original Python and the extracted worker consume every frozen P3a case."""
import importlib.util
import json
from pathlib import Path
import sys

from tl3_workers import pb_evidence


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


root = Path(sys.argv[1])
oracle = load('p3a_oracle', root / 'vector_oracle.py')
new = oracle.pb_tests, oracle.pb_generators, oracle.rehearsal
old_pb = load('p3a_old_pb', root / 'pb_tests_before.py')
old_generator = load('p3a_old_generator', root / 'pb_generators_before.py')
old_generator.TestSettings = old_pb.TestSettings
old_generator._failure_receipt = old_pb._failure_receipt
old_rehearsal = load('p3a_old_rehearsal', root / 'forcing_rehearsal_before.py')
old = old_pb, old_generator, old_rehearsal


def checker(command, document, **kwargs):
    print(json.dumps({'request': {'operation': command[1], **document}}), flush=True)
    return json.loads(sys.stdin.readline())


pb_evidence.run_check = checker
count = 0
for kind in oracle.KINDS:
    if kind == 'evidence-ledger':
        continue
    cases = json.loads((root.parent.parent.parent / 'pb-evidence/tests' / (kind + '.json')).read_text())['cases']
    for case in cases:
        for label, modules in [('original', old), ('Rust worker', new)]:
            oracle.pb_tests, oracle.pb_generators, oracle.rehearsal = modules
            actual = oracle.observe(kind, case['input'])
            assert oracle.encoded(actual) == oracle.encoded(case['output']), (kind, case['case'], label, str(case['output'])[:1600], str(actual)[:1600])
        count += 1
print(json.dumps({'done': count}), flush=True)
