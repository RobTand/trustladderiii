"""#734: the harness's golden check reports pass the worker's own reviewer-input validator.

The controller sends a parsed report's results to review unchanged (its contract-vector
tests assert that), and the worker validates its inputs again on stdin.
"""
import json
from pathlib import Path

from jsonschema import Draft202012Validator
import pytest

from tl3_workers.contracts import SCHEMA_BASE, registry

# released/ holds the previous release's reports, which review must still accept (#750 review).
VECTORS = sorted((Path(__file__).resolve().parents[2] / 'harness/test-vectors/contracts').rglob('*.json'))
CHECKS = Draft202012Validator({'$ref': SCHEMA_BASE + 'reviewer-input.json#/properties/checks'}, registry=registry())


def test_vectors_are_present():
    assert len(VECTORS) >= 7


@pytest.mark.parametrize('path', VECTORS, ids=lambda path: '/'.join(path.parts[-2:]))
def test_reviewer_checks_accept_the_harness_vector(path):
    results = json.loads(path.read_text())['results']
    errors = [f"{'/'.join(map(str, error.absolute_path))}: {error.message}" for error in CHECKS.iter_errors(results)]
    assert not errors, f'reviewer-input.json rejects {path.name}: {errors}'
