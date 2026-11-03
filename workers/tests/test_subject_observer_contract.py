"""Planning controls from the saved #645 corpus, including correct refusals."""
from copy import deepcopy

from tl3_workers.evidence import forcing_capabilities, preflight
from tl3_workers.generated_scope import output_freezes


def test_request230_planner_sees_the_authoritative_shared_delay_ceiling():
    capability = forcing_capabilities()
    assert capability.get('delay') == {'per_call_seconds': 5, 'per_test_seconds': 10,
                                       'shared_across': ['targets', 'forks'], 'reset': 'each pytest test'}


def test_request229_disk_environment_obligation_stays_a_named_refusal():
    capability = {'tool': 'run_tests', 'executor': 'prismabuild_pytest', 'runner_features': ['base_head'],
                  'artifacts': ['cas_receipt'], 'methods': ['red_green'], 'protected_files': []}
    requirement = {**deepcopy(capability), 'requirement': 'The original spool test needs local disk scratch.',
                   'runner_features': ['local_disk_scratch'],
                   'provenance': {'kind': 'source', 'source': 'issue.body', 'quote': 'The spool needs local disk.'}}
    reply = {'kind': 'leaf', 'files': [], 'checks': [], 'evidence_contract': [requirement]}
    preflight({'tool_capabilities': [capability]}, reply, planning=True)
    assert reply['kind'] == 'cant_meet'
    assert 'local_disk_scratch' in reply['report']
    assert requirement in reply['evidence_contract']


def test_request225_negated_generated_prohibition_does_not_invent_an_edit_ban():
    # tr-50cd0342: source prohibits hand editing, not configured regeneration.
    assert output_freezes('Do not hand-edit BUILD.bazel; configured regeneration is not prohibited.') == []
    assert output_freezes('Do not regenerate BUILD.bazel.')
