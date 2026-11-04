"""Source proof survives replacement of an unsuitable audit-selected method."""
from copy import deepcopy
from importlib.resources import files

import pytest

from jsonschema import Draft202012Validator

from tl3_workers.contracts import schema
from tl3_workers.evidence import forcing_capabilities, forcing_guidance, preflight
from tl3_workers.roles import planner


CAPABILITY = {'tool': 'run_tests', 'executor': 'prismabuild_pytest',
              'runner_features': ['base_head'], 'artifacts': ['cas_receipt', 'terminal_verdict'],
              'methods': ['red_green', 'forced_reproduction.environment', 'forced_reproduction.patches'],
              'protected_files': []}


def obligation(method, *, source=False):
    return {**deepcopy(CAPABILITY), 'methods': [method],
            'requirement': 'Show the real old assertion fails and the corrected expectation passes.',
            'provenance': {'kind': 'source', 'source': 'issue.body',
                           'quote': 'Use helper_target_observer.'} if source else {'kind': 'method'}}


def test_t613_ac1_source_required_method_and_provenance_cannot_be_replaced():
    required = obligation('helper_target_observer', source=True)
    reply = {'kind': 'leaf', 'evidence_contract': [obligation('red_green')]}
    preflight({'tool_capabilities': [CAPABILITY], 'ingest': {'evidence_contract': [required]}},
              reply, planning=True)
    assert reply['kind'] == 'cant_meet'
    assert required in reply['evidence_contract']
    supported = {**deepcopy(CAPABILITY), 'methods': [*CAPABILITY['methods'], 'helper_target_observer']}
    alternative = {'kind': 'leaf', 'evidence_contract': [obligation('red_green')], 'checks': []}
    preflight({'tool_capabilities': [supported], 'ingest': {'evidence_contract': [required]}},
              alternative, planning=True)
    assert alternative['kind'] == 'cant_meet'
    assert 'removed proof encoding helper_target_observer' in alternative['report']
    Draft202012Validator(schema('ingest-reply')['$defs']['evidence_requirement']).validate(required)


def test_t613_ac2_three_stage_reconciliation_replaces_methods_but_keeps_source():
    source = obligation('red_green', source=True)
    source['provenance']['quote'] = 'Show assertion RED on base and GREEN on head.'
    old = obligation('helper_target_observer')
    inputs = {'tool_capabilities': [CAPABILITY]}
    ingest = {'kind': 'brief', 'evidence_contract': [source, old]}
    preflight(inputs, ingest)
    audit = {'verdict': 'agree', 'objections': [], 'evidence_contract': [old]}
    preflight({**inputs, 'ingest': ingest}, audit)
    reply = {'kind': 'leaf', 'evidence_contract': [obligation('red_green')]}
    preflight({**inputs, 'ingest': ingest, 'audit_evidence_contract': audit['evidence_contract']},
              reply, planning=True)
    assert reply['kind'] == 'leaf', reply
    assert source in reply['evidence_contract']
    assert old not in reply['evidence_contract']
    assert reply['evidence_errors'] == []


@pytest.mark.parametrize('required_method', ['red_green', 'declared_symptom'])
def test_declared_symptom_choice_is_replaceable_and_source_proof_persists(required_method):
    capability = {**deepcopy(CAPABILITY), 'runner_features': ['base_head', 'repeat', 'parallel'],
                  'artifacts': ['cas_receipt', 'logs', 'sha', 'terminal_verdict'],
                  'methods': [*CAPABILITY['methods'], 'declared_symptom']}
    source = obligation(required_method, source=True)
    source['provenance']['quote'] = 'Prove the timeout.'
    inherited = obligation('helper_target_observer')
    inputs = {'tool_capabilities': [capability], 'ingest': {'evidence_contract': [source, inherited]},
              'audit_evidence_contract': [inherited]}
    reply = {'kind': 'leaf', 'evidence_contract': [], 'checks': [{'declared_symptom': {
        'node': 'tests/test_original.py::test_original', 'exception': 'builtins.TimeoutError',
        'phase': 'call', 'repeat': 2, 'parallel': 0}}]}
    preflight(inputs, reply, planning=True)
    assert reply['kind'] == 'leaf', reply
    assert reply['evidence_errors'] == []
    assert source in reply['evidence_contract'] and inherited not in reply['evidence_contract']
    chosen, = [entry for entry in reply['evidence_contract'] if entry != source]
    assert chosen['methods'] == ['declared_symptom']
    assert chosen['provenance'] == {'kind': 'method'}
    replacement = {'kind': 'leaf', 'evidence_contract': [], 'checks': []}
    preflight({**inputs, 'ingest': reply, 'audit_evidence_contract': reply['evidence_contract']},
              replacement, planning=True)
    assert replacement['kind'] == 'leaf', replacement
    assert replacement['evidence_contract'] == [source]


@pytest.mark.parametrize('body', ["return {'errno': 5}", 'assert False', 'return _failure()', "return {'validated': _failure()}"])
@pytest.mark.parametrize('decorated', [False, True])
def test_t613_ac4_helper_capability_does_not_establish_target_suitability(tmp_path, monkeypatch, body, decorated):
    (tmp_path / 'tests').mkdir()
    (tmp_path / 'pyproject.toml').write_text('[tool.tl3.harness]\ntest-files=["tests/*.py"]\n')
    (tmp_path / 'tests/test_evidence.py').write_text(
        'def _failure():\n    assert False\n\n'
        + ('@anything\n' if decorated else '') + f'def _eio():\n    {body}\n')
    monkeypatch.chdir(tmp_path)
    monkeypatch.setattr(planner.Forecaster, 'measure', lambda *args: {'fits': True})
    inputs = {'must_split': False, 'protected_paths': [], 'ingest': {'brief': 'Correct evidence.',
              'goals': [{'description': 'Fix evidence.', 'verification': 'test'}]},
              'forecast': {'fit_tokens': 100000, 'context_tokens': 150000, 'overhead_tokens': {}}}
    reply = {'kind': 'leaf', 'estimated_exploration_turns': 0,
             'files': ['tests/test_evidence.py'], 'edits_tests': [],
             'no_design_questions': True, 'lands_on_its_own': True,
             'checks': [{'kind': 'demonstrating', 'node_ids': ['tests/test_evidence.py::test_observer'],
                         'goal_ids': [0], 'proof': {'subjects': ['tests/test_evidence.py::_eio'],
                                                          'observer_support': [], 'red_message': 'wrong evidence'}}]}
    planner.check_reply(inputs, reply)
    assert reply['violations'] == []


def test_t613_ac5_path_depth_capability_is_explicitly_missing_and_names_stay_reserved():
    from tl3_harness import Forcing, HarnessInputError
    capabilities = forcing_capabilities()
    assert capabilities.get('path_depth', {}).get('supported') is False
    for name in ['TMPDIR', 'TEMP', 'TMP', 'PYTEST_DEBUG_TEMPROOT']:
        with pytest.raises(HarnessInputError):
            Forcing.parse({'symptom': 'Deep root.', 'environment': {name: '/tmp/deep'}})
    guidance = forcing_guidance()
    assert 'sandbox-local' in guidance and 'path-depth' in guidance


@pytest.mark.parametrize('role', ['ingest', 'audit', 'planner'])
def test_t613_ac6_refusal_and_selection_guidance_preserves_source_obligations(role):
    prompt = ' '.join(files('tl3_workers').joinpath('prompts', role + '.md').read_text().split())
    assert 'provenance' in prompt
    assert 'subject/observer rule' in prompt
    assert 'remaining source obligation' in prompt
    assert 'Do not substitute review or a flaky-test forcing for required red/green proof.' not in prompt
    assert 'If that staging would copy the helper fix onto base and erase RED, select helper_target_observer' not in prompt


def test_t613_missing_contract_has_a_safe_repair_diagnostic():
    from tl3_workers.tracing import failure_tags
    with pytest.raises(ValueError) as caught:
        preflight({'tool_capabilities': [CAPABILITY]}, {'kind': 'brief'})
    assert failure_tags(caught.value)['failure_message'] == (
        'Return evidence_contract with source requirements and separately marked method choices.')


def planner_prompt():
    return " ".join(files("tl3_workers").joinpath("prompts", "planner.md").read_text().split())


def test_planner_prompt_states_the_obligation_to_check_mapping():
    # PQ #3299 (#648 PR 3): the planner cited observer-new goals on regression checks and on
    # existing tests it edited, so proof applicability refused the completed plan.
    # test_each_stated_rule_is_what_proof_applicability_does checks each rule against the code.
    prompt = planner_prompt()
    for rule in (
        "Cite that goal only on demonstrating checks without declared_symptom.",
        "Never cite it on a regression or behavior-preserving check. A forcing is allowed.",
        "gives proof.red_message and covers every obligation subject in proof.subjects",
        "A subject is covered by itself or by its enclosing module or definition.",
        "every node_id of that check is a new test that the maker adds",
        "An existing test cannot prove it, also when the plan edits it.",
        'For observer "new", never cite the goal in rehearse_forcing goal_ids.',
        'For observer "any", an existing test can be the observer.',
        "rehearse_forcing goal_ids can cite it for an assertion campaign, never with declared_symptom.",
    ):
        assert rule in prompt, rule


SUBMIT = "tools/dispatch.py::Gateway::submit"
NEW = "tests/test_new.py::test_submit_reads_the_record"
OLD = "tests/test_old.py::test_edited_producer"
PROOF = {"subjects": [SUBMIT], "observer_support": [], "red_message": "submit returned another key"}
FORCING = {"symptom": "The record is late.", "environment": {"CASE": "late"}}
SYMPTOM = {"node": OLD, "exception": "builtins.TimeoutError", "phase": "call", "repeat": 2, "parallel": 0}


def _obligation_goal(observer):
    return {"description": "Migrate submit.", "verification": "test",
            "proof_obligation": {"kind": "assertion", "observer": observer, "subjects": [SUBMIT],
                                 "source": "issue.body", "quote": "Migrate submit."}}


# (prompt rule, observer, check, campaign, accepted). A campaign is a rehearse_forcing call.
RULES = [
    ("demonstrating", "new", {"kind": "demonstrating", "node_ids": [NEW], "proof": PROOF}, False, True),
    ("not regression", "any", {"kind": "regression", "node_ids": [OLD]}, False, False),
    ("not behavior-preserving", "any", {"kind": "behavior-preserving", "node_ids": [OLD]}, False, False),
    ("forcing allowed", "any", {"kind": "demonstrating", "node_ids": [OLD], "proof": PROOF,
                                "forcing": FORCING}, False, True),
    ("not declared_symptom", "any", {"kind": "demonstrating", "node_ids": [OLD], "proof": PROOF,
                                     "declared_symptom": SYMPTOM}, False, False),
    ("red_message", "new", {"kind": "demonstrating", "node_ids": [NEW],
                            "proof": {**PROOF, "red_message": ""}}, False, False),
    ("every subject", "new", {"kind": "demonstrating", "node_ids": [NEW],
                              "proof": {**PROOF, "subjects": ["tools/dispatch.py::Gateway::wait"]}}, False, False),
    ("enclosing module", "new", {"kind": "demonstrating", "node_ids": [NEW],
                                 "proof": {**PROOF, "subjects": ["tools/dispatch.py"]}}, False, True),
    ("enclosing definition", "new", {"kind": "demonstrating", "node_ids": [NEW],
                                     "proof": {**PROOF, "subjects": ["tools/dispatch.py::Gateway"]}}, False, True),
    ("new: no existing test", "new", {"kind": "demonstrating", "node_ids": [OLD], "proof": PROOF}, False, False),
    ("any: existing test", "any", {"kind": "demonstrating", "node_ids": [OLD], "proof": PROOF}, False, True),
    ("new: no rehearsal", "new", {"kind": "demonstrating", "node_ids": [OLD]}, True, False),
    ("any: assertion rehearsal", "any", {"kind": "demonstrating", "node_ids": [OLD]}, True, True),
    ("any: no symptom rehearsal", "any", {"kind": "demonstrating", "node_ids": [OLD],
                                          "declared_symptom": SYMPTOM}, True, False),
]


@pytest.mark.parametrize("rule, observer, check, campaign, accepted", RULES, ids=[rule[0] for rule in RULES])
def test_each_stated_rule_is_what_proof_applicability_does(rule, observer, check, campaign, accepted):
    from tl3_workers.evidence import proof_applicability

    errors = proof_applicability([_obligation_goal(observer)], [{**check, "goal_ids": [0]}],
                                 {OLD}.__contains__, campaign=campaign)
    assert (errors == []) is accepted, errors


def _pq3299_shaped_plan(cite_obligations_everywhere, campaign):
    """Goal 1 has an observer-new obligation; goal 2 keeps edited tests green."""
    goals = [{"description": "Add the adapter.", "verification": "test"}, _obligation_goal("new"),
             {"description": "Keep the named existing tests green.", "verification": "review"}]
    obligation = [1] if cite_obligations_everywhere else []
    if campaign:
        # The rehearse_forcing call that PQ #3299's planner made for edited existing tests.
        return goals, [{"kind": "demonstrating", "node_ids": [OLD], "goal_ids": [0, *obligation]}]
    checks = [
        {"kind": "demonstrating", "node_ids": [NEW], "goal_ids": [0, 1], "proof": PROOF},
        {"kind": "demonstrating", "node_ids": [OLD], "goal_ids": [2, *obligation], "proof": PROOF},
        {"kind": "regression", "node_ids": ["tests/test_old.py::test_unchanged_verdict"],
         "goal_ids": [2, *obligation]},
    ]
    return goals, checks


@pytest.mark.parametrize("campaign", [False, True], ids=["plan", "rehearsal"])
@pytest.mark.parametrize("follows_prompt", [False, True])
def test_the_prompt_mapping_is_what_proof_applicability_accepts(follows_prompt, campaign):
    from tl3_workers.evidence import proof_applicability

    goals, checks = _pq3299_shaped_plan(not follows_prompt, campaign)
    on_base = {OLD, "tests/test_old.py::test_unchanged_verdict"}
    errors = proof_applicability(goals, checks, on_base.__contains__, campaign=campaign)
    if follows_prompt:
        assert errors == []
    else:
        # The refusals PQ #3299's planner trace received.
        assert any("requires an assertion observer" in e for e in errors), errors
        if not campaign:
            assert any("requires a new deterministic observer; an existing base test is insufficient" in e
                       for e in errors), errors
