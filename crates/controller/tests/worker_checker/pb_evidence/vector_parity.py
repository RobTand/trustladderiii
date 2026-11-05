"""Both original Python and the extracted worker consume every frozen P3a case."""
import importlib.util
import json
from pathlib import Path
import sys

from tl3_workers import pb_tests as pb_evidence


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


root = Path(sys.argv[1])
repo = root.parent.parent.parent.parent
sys.path.insert(0, str(repo / 'workers/tests'))
import pb_evidence_vectors as oracle
import pb_evidence_reference as reference
new = oracle.pb_tests, oracle.pb_generators, oracle.rehearsal
old = reference.pb_tests, reference.pb_generators, reference.forcing_rehearsal


def checker(command, document, **kwargs):
    print(json.dumps({'request': {'operation': command[1], **document}}), flush=True)
    return json.loads(sys.stdin.readline())


pb_evidence.run_check = checker
count = 0
for kind in (sys.argv[2],):
    cases = json.loads((repo / 'workers/tests/pb-evidence-vectors' / (kind + '.json')).read_text())['cases']
    for index, case in enumerate(cases):
        if index % int(sys.argv[4]) != int(sys.argv[3]):
            continue
        for label, modules in [('original', old), ('Rust worker', new)]:
            oracle.pb_tests, oracle.pb_generators, oracle.rehearsal = modules
            actual = oracle.observe(kind, case['input'])
            assert oracle.encoded(actual) == oracle.encoded(case['output']), (kind, case['case'], label, str(case['output'])[:1600], str(actual)[:1600])
        count += 1
print(json.dumps({'done': count}), flush=True)
