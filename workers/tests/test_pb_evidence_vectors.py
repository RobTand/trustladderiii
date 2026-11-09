"""The committed native evidence corpus is an in-process oracle for the Rust port (#821)."""
import json

from pb_evidence_vectors import KINDS, check_coverage, check_vectors, read_vectors


def test_every_refusal_has_a_case():
    check_coverage(read_vectors())


def test_current_python_matches_committed_vectors():
    vectors = check_vectors()
    assert set(vectors) == set(KINDS)
    for case in vectors['evidence-ledger']:
        if 'raise' in case['output']:
            continue
        result = case['output']['return']
        assert result['state'] == result['restored'], case['case']
        assert len(result['state']['test_runs']) <= 10, case['case']
        assert len(json.dumps(result['state']['test_runs']).encode()) <= 65_536, case['case']


def test_payload_corruptions_reject_before_a_verdict():
    cases = {case['case']: case['output'] for case in read_vectors()['payload-summary']}
    accepted = cases['executed']['return']
    assert accepted['status'] == 'completed'
    assert accepted['wait_reference'].startswith('worker:') and accepted['receipts'][0]['backend'] == 'flux'
    assert set(accepted['receipts'][0]) == {'backend', 'wait_reference', 'run_binding', 'receipt_sha256', 'ending'}
    for name in ('shape-status', 'shape-tests', 'shape-note', 'shape-totals', 'totals-keys', 'totals-negative',
                 'totals-bool', 'totals-float', 'test-shape', 'test-name', 'test-outcome'):
        assert cases[name]['raise'] == {'type': 'ValueError', 'message': 'Invalid summary structure'}, name
    assert cases['symptom-missing']['raise']['message'] == 'missing symptom run receipt'
    for name in ('symptom-run-index', 'symptom-run-sha', 'symptom-run-summary'):
        assert cases[name]['raise']['message'] == 'symptom run receipt changed', name
    assert cases['payload-bytes-changed']['raise']['message'] == 'Verified worker payload bytes changed'
    assert cases['payload-over-bound']['raise']['message'] == 'Verified worker payload exceeds its generated bound'
    for name in ('foreign-binding', 'null-binding'):
        assert 'another run binding' in cases[name]['raise']['message'], name
    assert cases['not-passed']['raise']['message'] == 'Shared execution retained an unknown or refused outcome'


def test_post_wait_consistency_classifies_each_mismatch():
    cases = {case['case']: case['output'] for case in read_vectors()['post-wait-consistency']}
    assert cases['consistent']['return']['status'] == 'completed'
    assert cases['compact-selection']['return']['status'] == 'completed'
    for name, reason in [('proof-mismatch', 'proof_mismatch'), ('forcing-mismatch', 'forcing_or_selection_mismatch'),
                         ('selection-mismatch', 'forcing_or_selection_mismatch'), ('symptom-mismatch', 'symptom_mismatch')]:
        result = cases[name]['return']
        assert result['status'] == 'infrastructure_error'
        assert result['execution']['termination_reason'] == reason


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
    cases = {case['case']: case['output']['return'] for case in read_vectors()['evidence-ledger'] if 'return' in case['output']}
    recovered = cases['infra-retirement-and-roundtrip']
    assert recovered['steps'][0]['infrastructure_keys']
    assert recovered['state']['infrastructure_keys'] == []
    assert all(not citation['cites'] for citation in recovered['citations'])
    assert cases['base-does-not-retire-head']['citations'][0]['cites'] is True
    newest = cases['ten-entries-newest-kept']['state']['test_runs']
    assert len(newest) == 10 and newest[-1]['tree'] == f'{11:040x}'
    bounded = cases['byte-bound-and-selector-identity']['state']['test_runs']
    assert bounded[-1]['tests'][0].startswith('test_4_')
    assert bounded[-1]['tests_identity']['count'] == 256
    assert bounded[-1]['tests_omitted'] > 0


def test_summary_container_and_root_errors_are_preserved():
    cases = {case['case']: case['output'] for case in read_vectors()['payload-summary']}
    for shape, cls in [('null', 'NoneType'), ('list', 'list'), ('string', 'str'), ('int', 'int'), ('bool', 'bool')]:
        assert cases['root-' + shape]['raise'] == {'type': 'AttributeError', 'message': f"'{cls}' object has no attribute 'get'"}


def test_summary_bounds_use_the_enriched_python_json_bytes():
    cases = {case['case']: case['output'] for case in read_vectors()['payload-summary']}
    for name, size in [('below', 15871), ('at', 15872), ('above', 15873)]:
        result = cases['summary-bytes-' + name]['return']
        assert result['before_bytes'] == size
        assert result['after_bytes'] <= 15872
        summary = result['summary']
        assert summary['totals']['passed'] == 1 and summary['receipts'][0]['wait_reference']
        # These raw summaries already exceed the former action limit, before
        # receipts are attached. #823 now enforces that limit in Rust first.
        assert summary['tests'] == [] and summary['note'].startswith('Truncated:')
    for name, size in [('below', 14335), ('at', 14336), ('above', 14337)]:
        result = cases['action-summary-bytes-' + name]['return']
        assert result['before_bytes'] == size and result['after_bytes'] <= 15872
        summary = result['summary']
        assert summary['totals']['passed'] == 1 and summary['receipts'][0]['wait_reference']
        if name == 'above':
            assert summary['tests'] == [] and summary['note'].startswith('Truncated:')
        else:
            assert len(summary['tests']) == 1 and summary['note'] == ''


def test_real_generator_limits_and_text_refusals():
    cases = {case['case']: case for case in read_vectors()['generator-artifact']}
    for prefix, limit in [('file-bytes', 16777216), ('total-bytes', 33554432), ('outputs', 128)]:
        for suffix, offset in [('below', -1), ('at', 0), ('above', 1)]:
            case = cases[prefix + '-' + suffix]
            sizes = case['input']['sizes']
            actual = len(sizes) if prefix == 'outputs' else sum(sizes)
            assert actual == limit + offset
            if suffix == 'above':
                expected = {'file-bytes': 'generator output exceeds the file size limit',
                            'total-bytes': 'generator output exceeds total size limit',
                            'outputs': 'generator output file count exceeds limit'}[prefix]
                assert case['output']['raise'] == {'type': 'ValueError', 'message': expected}
            else:
                result = case['output']['return']
                assert result['outputs'] == len(sizes) and result['bytes'] == sum(sizes)
    assert cases['control-text']['output']['raise'] == {'type': 'ValueError', 'message': 'generator output must be text'}
    assert cases['invalid-utf8']['output']['raise']['type'] == 'UnicodeDecodeError'
    assert cases['validate-failed']['output']['raise'] == {'type': 'ValueError', 'message': 'fixture generator failed'}


def test_invalid_ledger_selectors_and_state_keep_their_refusals():
    cases = {case['case']: case['output'] for case in read_vectors()['evidence-ledger']}
    for name in ('invalid-selectors-base', 'invalid-selectors-test'):
        assert cases[name]['raise'] == {'type': 'ValueError', 'message': 'invalid evidence selectors'}
    for name in ('invalid-state-key', 'invalid-state-tool', 'invalid-state-runs', 'invalid-state-run'):
        assert cases[name]['raise'] == {'type': 'ValueError', 'message': 'invalid evidence state'}
