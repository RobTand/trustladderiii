"""Verbatim shells at rust-fv 8345c03926; remove after #821 ships."""
import importlib.util
from pathlib import Path
import sys


def load(name):
    path = Path(__file__).with_name('pb-evidence-reference') / (name + '.py')
    spec = importlib.util.spec_from_file_location('pb_evidence_reference.' + name, path)
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


pb_tests = load('pb_tests')
pb_generators = load('pb_generators')
pb_generators.TestSettings = pb_tests.TestSettings
pb_generators._failure_receipt = pb_tests._failure_receipt
forcing_rehearsal = load('forcing_rehearsal')

# S2 changed the action envelope in the integration base. Keep the three original
# references verbatim and separately pin that current-base summary shell.
s2_pb_tests = load('s2_pb_tests')
from tl3_workers import pb_tests as current
s2_pb_tests._harness_validator = current._harness_validator
pb_tests._summary = s2_pb_tests._summary
