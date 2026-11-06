"""Run paired corpus calls only inside a PB measurement action."""
import base64
import json
from pathlib import Path
import statistics
import subprocess
import sys
import tempfile
import time
from types import SimpleNamespace

sys.path.insert(0, str(Path('../../harness/src').resolve()))
from tl3_harness import model, proof

binary, corpus = sys.argv[1:]
rows = [json.loads(line) for line in Path(corpus).read_text().splitlines()]
measurements = []
with subprocess.Popen([binary], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, bufsize=1) as rust:
    for index, row in enumerate(rows):
        with tempfile.TemporaryDirectory(prefix='spike902-pair-') as temporary:
            root = Path(temporary)
            data = row['input']
            for side in ('base', 'head'):
                (root / side).mkdir()
                for name, value in data[side].items():
                    path = root / side / name
                    path.parent.mkdir(parents=True, exist_ok=True)
                    path.write_bytes(base64.b64decode(value['b64']))
                    path.chmod(value['mode'])
            destination = root / 'staged'
            if data['destination_exists']:
                destination.mkdir()
            declaration = data['check']
            check = SimpleNamespace(**{**declaration, 'kind': model.Kind(declaration['kind']),
                'tests': tuple(declaration['tests']), 'proof': model.Proof.parse(declaration['proof']) if declaration['proof'] else None,
                'forcing': model.Forcing.parse(declaration['forcing']) if declaration['forcing'] else None})
            def python_call():
                started = time.perf_counter_ns()
                try:
                    receipt = proof.assemble_proof(root / 'base', root / 'head', destination, check, data['config'], **data['kwargs'])
                    result = {'accepted': True, 'receipt': receipt}
                except model.ProofRefusal as error:
                    result = {'accepted': False, 'rule': error.rule}
                return time.perf_counter_ns() - started, result
            def rust_call():
                rust.stdin.write(json.dumps(row) + '\n')
                rust.stdin.flush()
                result = json.loads(rust.stdout.readline())
                return result['rust_ns'], result
            # Alternate order to reduce monotonic drift across the corpus.
            if index % 2:
                rust_ns, rust_result = rust_call()
                python_ns, python_result = python_call()
            else:
                python_ns, python_result = python_call()
                rust_ns, rust_result = rust_call()
            expected = row['python']
            baseline_equal = python_result['accepted'] == expected['accepted'] and (
                python_result['receipt'] == expected['receipt'] if expected['accepted'] else python_result['rule'] == expected['rule'])
            measurements.append({'id': row['id'], 'python_ns': python_ns, 'rust_ns': rust_ns, 'python_baseline_equal': baseline_equal})
    rust.stdin.close()
    rust.wait()
    if rust.returncode:
        raise SystemExit(rust.returncode)
def stats(key):
    values = sorted(row[key] / 1000 for row in measurements)
    return {'n': len(values), 'median_us': statistics.median(values), 'p95_us': values[int(.95 * len(values))]}
print(json.dumps({'python': stats('python_ns'), 'rust': stats('rust_ns'),
                  'python_baseline_equal': sum(row['python_baseline_equal'] for row in measurements),
                  'caveat': 'Same worker and interleaved calls. Python stages filesystem trees; Rust operates on in-memory maps. Startup, serialization, input materialization, and output hashing are excluded.',
                  'measurements': measurements}))
