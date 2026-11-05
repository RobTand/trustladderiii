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


def test_ending_corruptions_reject_at_the_original_boundary():
    cases = {case['case']: case['output'] for case in read_vectors()['ending-summary']}
    assert cases['executed']['return']['status'] == 'completed'
    assert cases['cache-hit']['return']['receipts'][0]['status'] == 'cache_hit'
    for name in ('key', 'status', 'exit', 'succeeded', 'verified', 'receipt-sha', 'receipt-type',
                 'log-size-negative', 'log-size-bool', 'log-size-limit', 'log-size-changed',
                 'log-sha', 'no-summary', 'two-summaries', 'json', 'shape-status', 'shape-tests',
                 'shape-note', 'shape-totals', 'totals-keys', 'totals-negative', 'totals-bool',
                 'totals-float', 'test-shape', 'test-name', 'test-outcome', 'symptom-missing',
                 'symptom-run-index', 'symptom-run-sha', 'symptom-run-summary'):
        error = cases[name]['raise']
        assert error['type'] == 'ExecutionFailure', name
        assert error['execution']['outcome'] == 'transport_failure', name
        assert error['execution']['phase'] == 'summary', name


def test_failed_record_matches_publication_and_filters_private_metadata():
    cases = {case['case']: case['output']['return'] for case in read_vectors()['failure-receipt']}
    assert cases['published-record']['termination_reason'] == 'memory_limit_oom'
    for name in ('wrong-key', 'wrong-publication', 'invalid-record-json', 'record-over-limit'):
        assert cases[name]['termination_reason'] == 'worker_lost'
    assert 'private_metadata' not in json.dumps(cases)
    assert 'termination_reason' not in cases['reason-unsafe']
    assert 'memory_peak_bytes' not in cases['peak-bool']


def test_post_wait_consistency_classifies_each_mismatch():
    cases = {case['case']: case['output'] for case in read_vectors()['post-wait-consistency']}
    assert cases['consistent']['return']['status'] == 'completed'
    assert cases['compact-selection']['return']['status'] == 'completed'
    for name, reason in [('proof-mismatch', 'proof_mismatch'), ('forcing-mismatch', 'forcing_or_selection_mismatch'),
                         ('selection-mismatch', 'forcing_or_selection_mismatch'), ('symptom-mismatch', 'symptom_mismatch')]:
        result = cases[name]['return']
        assert result['status'] == 'infrastructure_error'
        assert result['execution']['termination_reason'] == reason


def test_capability_and_stdout_paths_keep_their_transport_rules():
    cases = {case['case']: case['output']['return'] for case in read_vectors()['machine-record-fallback']}
    supported = cases['supported-private-wins']
    assert supported['value'][1] == {'source': 'private'}
    assert supported['calls'] == ['probe', 'private']
    assert supported['fallbacks'] == []
    for name in ('absent', 'bool', 'float', 'zero', 'exit-2-fallback', 'probe-io-fallback'):
        assert cases[name]['value'][1] == {'source': 'stdout'}, name
        assert cases[name]['calls'] == ['probe', 'stdout'], name
        assert cases[name]['fallbacks'] == ['pbrun'], name
    assert cases['private-empty']['error']['execution']['termination_reason'] == 'machine_record_empty'
    assert cases['private-empty']['calls'] == ['probe', 'private']


def test_generator_verification_imports_only_the_verified_artifact():
    cases = {case['case']: case['output'] for case in read_vectors()['generator-artifact']}
    accepted = cases['verified-import']['return']
    assert accepted['result']['status'] == 'completed'
    assert accepted['files'] == {'tests/BUILD.bazel': '# generated\n'}
    for name in ('stale-input', 'argv', 'count', 'digest', 'mode', 'output-manifest', 'path-traversal',
                 'reserved-path', 'undeclared', 'identity', 'budget', 'command', 'duplicate-output',
                 'protected-output', 'unchanged-output'):
        result = cases[name]['return']
        assert result['result']['status'] == 'failed', name
        assert not result['files'] or name == 'unchanged-output', name


def test_identity_preserves_python_json_and_permitted_reordering():
    cases = {case['case']: case['output']['return'] for case in read_vectors()['rehearsal-identity']}
    original = cases['unicode-int-float']
    assert original['canonical'] == cases['key-order']['canonical']
    assert original['execution_identity'] == cases['selector-and-independent-patch-order']['execution_identity']
    assert original['bound_identity'] != cases['selector-and-independent-patch-order']['bound_identity']
    assert '\\u00e9' in original['canonical'] and 'café' in original['python_sorted_default']
    assert '1.0' in original['canonical'] and '1e-07' in cases['exponent']['canonical']
    assert cases['violations-matched']['violations'] == []
    assert cases['violations-absent']['violations']


def test_ledger_bounds_retirement_and_newest_entry():
    cases = {case['case']: case['output']['return'] for case in read_vectors()['evidence-ledger']}
    recovered = cases['infra-retirement-and-roundtrip']
    assert recovered['steps'][0]['infrastructure_keys']
    assert recovered['state']['infrastructure_keys'] == []
    assert all(not citation['cites'] for citation in recovered['citations'])
    assert cases['base-does-not-retire-head']['citations'][0]['cites'] is True
    assert cases['cache-hit']['state']['test_runs'][0]['fresh'] is False
    newest = cases['ten-entries-newest-kept']['state']['test_runs']
    assert len(newest) == 10 and newest[-1]['tree'] == f'{11:040x}'
    bounded = cases['byte-bound-and-selector-identity']['state']['test_runs']
    assert bounded[-1]['tests'][0].startswith('test_4_')
    assert bounded[-1]['tests_identity']['count'] == 256
    assert bounded[-1]['tests_omitted'] > 0
