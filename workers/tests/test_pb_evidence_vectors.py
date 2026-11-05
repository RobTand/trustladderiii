"""The committed PB corpus is an in-process oracle for the Rust port (#821)."""
import json

from pb_evidence_vectors import KINDS, check_coverage, check_vectors, read_vectors


def test_every_refusal_has_a_case():
    check_coverage(read_vectors())


def test_current_python_matches_committed_vectors():
    vectors = check_vectors()
    assert set(vectors) == set(KINDS)
    for case in vectors['evidence-ledger']:
        result = case['output']['return']
        assert result['state'] == result['restored'], case['case']
        assert len(result['state']['test_runs']) <= 10, case['case']
        assert len(json.dumps(result['state']['test_runs']).encode()) <= 65_536, case['case']
