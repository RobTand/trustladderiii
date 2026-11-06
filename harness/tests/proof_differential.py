"""Compare every existing proof case with the original assembly verdict and site."""
import json
from pathlib import Path
import shutil
import types

import pytest

from tl3_harness import execution, proof
from tl3_harness.model import ProofRefusal


FIXTURES = Path(__file__).parent / 'fixtures/subject_observer'
SITES = {item['line']: item['rule'] for item in json.loads((FIXTURES / 'proof_refusal_sites.json').read_text())}
REFERENCE = FIXTURES / 'proof_before_r1.py.txt'


class OriginalRefusal(Exception):
    def __init__(self, reason, module='', symbol=''):
        self.reason, self.module, self.symbol = reason, module, symbol


original = types.ModuleType('tl3_harness.proof_before_r1')
original.__package__ = 'tl3_harness'
exec(compile(REFERENCE.read_text(), str(REFERENCE), 'exec'), original.__dict__)
original.ProofRefusal = OriginalRefusal


def outcome(function, args, kwargs):
    try:
        return ('accepted', function(*args, **kwargs))
    except OriginalRefusal as error:
        trace = error.__traceback__
        line = None
        while trace:
            if trace.tb_frame.f_code.co_filename == str(REFERENCE):
                line = trace.tb_lineno
            trace = trace.tb_next
        return (SITES[line], None)
    except SyntaxError:
        return ('proof.source.unparsable', None)


@pytest.fixture(autouse=True)
def preserve_proof_verdicts(request, monkeypatch, tmp_path):
    """Existing scenarios compare both acceptance and the original refusing site."""
    if request.node.name.startswith('test_r1_'):
        return
    calls = []
    current_assembly, current_support = proof.assemble_proof, proof.validate_support

    def compare(function, old_function, args, kwargs, *, assembly=False):
        reference_args = args
        reference_destination = None
        if assembly:
            reference_destination = tmp_path / f'original-proof-{len(calls)}'
            if args[2].exists():
                reference_destination.mkdir(exist_ok=True)
            reference_args = (*args[:2], reference_destination, *args[3:])
        expected, reference_result = outcome(old_function, reference_args, {k: v for k, v in kwargs.items() if k != 'role'})
        calls.append((expected, old_function.__name__))
        return_refusal = False
        try:
            actual = function(*args, **kwargs)
        except ProofRefusal as error:
            assert expected != 'accepted', f'accept/refuse flip: {request.node.nodeid}: {error}'
            assert error.rule == expected, (request.node.nodeid, expected, error.rule)
            return_refusal = True
            raise
        else:
            return_refusal = False
            assert expected == 'accepted', f'refuse/accept flip: {request.node.nodeid}: {expected}'
            if assembly:
                assert actual == reference_result
                def files(root):
                    return {path.relative_to(root).as_posix(): path.read_bytes()
                            for path in root.rglob('*') if path.is_file()}
                assert files(args[2]) == files(reference_destination)
            return actual
        finally:
            if reference_destination and reference_destination.exists():
                shutil.rmtree(reference_destination)
            print('R1_PARITY ' + json.dumps({'case': request.node.nodeid, 'call': len(calls),
                                            'function': old_function.__name__, 'expected': expected,
                                            'refused': return_refusal}))

    def assembly(*args, **kwargs):
        return compare(current_assembly, original.assemble_proof, args, kwargs, assembly=True)

    def support(*args, **kwargs):
        return compare(current_support, original.validate_support, args, kwargs)

    monkeypatch.setattr(proof, 'assemble_proof', assembly)
    monkeypatch.setattr(execution, 'assemble_proof', assembly)
    monkeypatch.setattr(proof, 'validate_support', support)
    if hasattr(request.module, 'assemble_proof'):
        monkeypatch.setattr(request.module, 'assemble_proof', assembly)
