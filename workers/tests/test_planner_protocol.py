"""Planner protocol contracts."""
# ruff: noqa: F811

import copy
from pathlib import Path

import pytest

from planner_fixtures import (
    CANT_MEET,
    LEAF,
    SPLIT,
    assert_rejected,
    assert_rule_report,
    cache_tokenizer,  # noqa: F401
    planner_request,  # noqa: F401
    run_reply,
    run_reply_in_process,
    tool_capable_fake,  # noqa: F401
)


def test_out_of_room_rejects_leaf(tmp_path, planner_request, monkeypatch):
    from tl3_workers.roles.planner import check_reply

    planner_request["inputs"]["must_split"] = True
    monkeypatch.chdir(planner_request["worktree"])
    reply = copy.deepcopy(LEAF)
    check_reply(planner_request["inputs"], reply)
    assert "Must split after running out of room." in reply["violations"]


def test_out_of_room_leaf_is_rejected_through_protocol(tmp_path, planner_request):
    planner_request["inputs"]["must_split"] = True
    assert_rule_report(run_reply_in_process(tmp_path, planner_request, LEAF), "Must split")


@pytest.mark.parametrize("proposal", [SPLIT, CANT_MEET])
def test_out_of_room_can_split_or_report_obstacle(tmp_path, planner_request, proposal):
    planner_request["inputs"]["must_split"] = True
    planner_request["inputs"]["findings"] = ["The maker exhausted its context."]
    proposal = copy.deepcopy(proposal)
    if proposal["kind"] == "split":
        proposal["reason"] = "out_of_room"
    result = run_reply_in_process(tmp_path, planner_request, proposal)
    assert result.exit_code == 0, result.stderr
    assert result.events[-1]["reply"]["kind"] == proposal["kind"]


def test_protected_path_is_rejected_through_protocol(tmp_path, planner_request):
    assert_rule_report(run_reply_in_process(tmp_path, planner_request, {**LEAF, "files": ["conftest.py"]}), "protected path")


@pytest.mark.parametrize("field", ["must_split", "findings", "protected_paths"])
def test_controller_must_supply_planning_inputs(tmp_path, planner_request, field):
    del planner_request["inputs"][field]
    assert_rejected(run_reply_in_process(tmp_path, planner_request, LEAF), "request")


@pytest.mark.parametrize("proposal", [LEAF, SPLIT])
@pytest.mark.parametrize("path,pattern", [
    (".github/workflows/test.yml", ".github/**"),
    (".mergify.yml", ".mergify.yml"),
    ("conftest.py", "**/conftest.py"),
    ("tests/nested/conftest.py", "**/conftest.py"),
    ("pytest.ini", "pytest.ini"),
    ("tox.ini", "tox.ini"),
    ("noxfile.py", "noxfile.py"),
    ("setup.cfg", "setup.cfg"),
    ("pyproject.toml", "pyproject.toml"),
    ("src/private.py", "src/*.py"),
])
def test_protected_paths_are_rejected(tmp_path, planner_request, monkeypatch, proposal, path, pattern):
    from tl3_workers.roles.planner import check_reply

    planner_request["inputs"]["protected_paths"] = [pattern]
    reply = copy.deepcopy(proposal)
    if reply["kind"] == "leaf":
        reply["files"] = [path]
    else:
        reply["pieces"][1]["files"] = [path]
    monkeypatch.chdir(planner_request["worktree"])
    check_reply(planner_request["inputs"], reply)
    assert any("protected path" in report for report in reply["violations"])


@pytest.mark.parametrize("change,reported", [
    ({"checks": []}, True),
    ({"checks": [LEAF["checks"][0]]}, True),
    ({"checks": [{**LEAF["checks"][0], "goal_ids": [0, 1, 2]}]}, True),
    ({"checks": [{**LEAF["checks"][0], "goal_ids": []}]}, True),
    ({"checks": [{**LEAF["checks"][0], "node_ids": []}]}, True),
    ({"checks": [{**LEAF["checks"][0], "kind": "guess"}]}, False),
    ({"checks": [{**LEAF["checks"][0], "node_ids": ["tests/test_greeting.py"]}]}, True),
    ({"files": []}, True),
    ({"files": ["greeting.py", "greeting.py"]}, False),
    ({"no_design_questions": False}, True),
    ({"lands_on_its_own": False}, True),
])
def test_leaf_rules_are_reported_and_malformed_shapes_fail(tmp_path, planner_request, change, reported):
    result = run_reply_in_process(tmp_path, planner_request, {**LEAF, **change})
    if reported:
        assert_rule_report(result)
    else:
        assert_rejected(result)


def test_invalid_node_id_is_reported_to_controller(tmp_path, planner_request):
    proposal = copy.deepcopy(LEAF)
    proposal["checks"][0]["node_ids"] = ["../test.py::test_escape"]
    result = run_reply_in_process(tmp_path, planner_request, proposal)
    assert_rule_report(result)
    assert result.events[-1]["reply"]["violations"][:1] == [
        "Checks must name exact pytest node IDs. Replace `../test.py::test_escape` "
        "with the exact test definitions this check needs."]


@pytest.mark.parametrize("path", [
    "../outside.py", "/etc/passwd", "new/../../outside.py", "new/../outside.py",
    "new\\outside.py", ".git/config", "new/.git/config", "./greeting.py",
    "new//file.py", "greeting.py/", ".", "bad\x00path",
])
def test_declared_paths_are_confined(tmp_path, planner_request, path):
    result = run_reply_in_process(tmp_path, planner_request, {**LEAF, "files": [path]})
    # #321: a named violation, not a crash. Nothing outside the worktree is measured.
    assert_rule_report(result, f"Declared file can't be used: {path}: ")
    assert result.events[-1]["reply"]["forecast"]["file_tokens"][path] == 0


@pytest.mark.parametrize("proposal,reported", [
    ({**SPLIT, "pieces": SPLIT["pieces"][:1]}, True),
    ({**SPLIT, "reason": "convenience"}, False),
    ({**SPLIT, "pieces": [SPLIT["pieces"][0], SPLIT["pieces"][0]]}, True),
    ({**SPLIT, "pieces": [SPLIT["pieces"][0], {**SPLIT["pieces"][1], "goal_ids": [99]}]}, True),
    ({**SPLIT, "pieces": [SPLIT["pieces"][0], {**SPLIT["pieces"][1], "files": ["../escape"]}]}, True),
    ({**CANT_MEET, "report": ""}, False),
    ({**CANT_MEET, "files": ["../escape"]}, True),
])
def test_split_rules_are_reported_and_malformed_shapes_fail(tmp_path, planner_request, proposal, reported):
    result = run_reply_in_process(tmp_path, planner_request, proposal)
    if reported:
        assert_rule_report(result)
    else:
        assert_rejected(result)


def test_early_ingest_end_is_not_plannable(tmp_path, planner_request):
    planner_request["inputs"]["ingest"] = {
        "kind": "early_end", "outcome": "duplicate", "reason": "Another request covers it.",
    }
    assert_rejected(run_reply_in_process(tmp_path, planner_request, LEAF), "request")


def test_read_only_tools_through_protocol(tmp_path, planner_request):
    calls = [
        {"name": name, "args": args, "id": name, "type": "tool_call"}
        for name, args in [
            ("list", {}), ("read", {"path": "greeting.py"}),
            ("glob", {"pattern": "*.py"}), ("grep", {"pattern": "hello"}),
            ("write", {"path": "forbidden.py", "content": "no"}),
            ("edit", {"path": "greeting.py", "old": "hello", "new": "changed"}),
        ]
    ]
    result = run_reply(tmp_path, planner_request, copy.deepcopy(LEAF), calls)
    assert result.exit_code == 0, result.stderr
    assert result.events[:-1] == [
        {"event": "progress", "unit": "model_turn"},
        *[{"event": "progress", "unit": "tool_result"} for _ in calls],
        {"event": "progress", "unit": "model_turn"},
    ]
    assert not (tmp_path / "worktree/forbidden.py").exists()
    assert (tmp_path / "worktree/greeting.py").read_text() == "hello world\n"
    assert result.events[-1]["reply"]["forecast"]["file_tokens"]["greeting.py"] == 3


def test_model_cannot_forge_rule_evidence(tmp_path, planner_request):
    assert_rule_report(run_reply_in_process(tmp_path, planner_request, {**LEAF, 'violations': []}), 'only code may supply planning evidence')


def test_split_requires_evidence_and_smaller_children(tmp_path, planner_request):
    reply = run_reply_in_process(tmp_path, planner_request, SPLIT)
    assert_rule_report(reply, "forecast fits")
    assert_rule_report(reply, "smaller forecast")


@pytest.mark.parametrize("pattern", ["tests/BUILD.bazel", "**/BUILD.bazel"])
def test_request_2709_plan_reserves_generated_outputs(planner_request, monkeypatch, pattern):
    from tl3_workers.roles.planner import check_reply
    monkeypatch.chdir(planner_request["worktree"])
    Path("tests/BUILD.bazel").write_text("# Generated by configured generator\n")
    config = planner_request["inputs"]["forecast"]
    config.update(generator="python3 generate.py", generated_files=[pattern], generator_available=True)
    reply = copy.deepcopy(LEAF)
    manual = list(reply["files"])
    check_reply(planner_request["inputs"], reply)
    assert not reply["violations"], reply
    assert reply["files"] == sorted([*manual, "tests/BUILD.bazel"])
    assert reply["forecast"]["file_tokens"]["tests/BUILD.bazel"] == 1024


def test_generated_footprint_never_reserves_protected_output(planner_request, monkeypatch):
    from tl3_workers.roles.planner import check_reply
    monkeypatch.chdir(planner_request["worktree"])
    Path("tests/BUILD.bazel").write_text("# Generated by configured generator\n")
    planner_request["inputs"]["forecast"].update(
        generator="python3 generate.py", generated_files=["**/BUILD.bazel"], generator_available=True)
    planner_request["inputs"]["protected_paths"].append("tests/BUILD.bazel")
    reply = copy.deepcopy(LEAF)
    check_reply(planner_request["inputs"], reply)
    assert "tests/BUILD.bazel" not in reply["files"]


def _named_run(planner_request, monkeypatch, draft, candidate=None):
    from planner_fixtures import planner_model
    from tl3_workers import runtime

    monkeypatch.chdir(planner_request['worktree'])
    planner_request['inputs']['repair_plan_consistency'] = True
    model = planner_model(monkeypatch, draft, candidate if candidate is not None else draft)
    events = []
    runtime.run(planner_request, events.append)
    return model, events[-1]


@pytest.mark.parametrize('change', ['unchanged', 'honest_stop', 'unrelated', 'fixed'])
def test_t832_q63_proof_message_gets_one_named_repair(planner_request, monkeypatch, change):
    # Live Q63 is audit-origin. This exercises its planner analogue with synthetic
    # file facts and the exact assertion text retained in the source declaration.
    draft = copy.deepcopy(LEAF)
    assertion = '_git must run every Git call with maintenance.auto=false and gc.auto=0'
    planner_request['inputs']['ingest']['goals'][0]['proof_obligation'] = {
        'kind': 'assertion', 'observer': 'new', 'subjects': ['greeting.py::_git'],
        'source': 'issue.body', 'quote': assertion}
    draft['checks'] = [copy.deepcopy(draft['checks'][0])]
    draft['checks'][0]['goal_ids'] = [0, 1]
    draft['checks'][0]['proof'] = {'subjects': ['greeting.py::_git'], 'observer_support': [], 'red_message': ''}
    fixed = copy.deepcopy(draft)
    if change in {'fixed', 'unrelated'}:
        fixed['checks'][0]['proof']['red_message'] = assertion
    if change == 'unrelated':
        fixed['checks'][0]['node_ids'] = ['tests/test_greeting.py::test_unrelated']
    if change == 'honest_stop':
        fixed = copy.deepcopy(CANT_MEET)
    model, event = _named_run(planner_request, monkeypatch, draft, fixed)
    assert len(model.calls) == 2
    history, tools = model.calls[-1]
    assert not tools
    assert 'proof.red_missing' in history[-1].text
    assert '/checks/0/proof/red_message' in history[-1].text
    if change == 'fixed':
        assert event['reply']['violations'] == []
        assert 'plan_check' not in event
    else:
        provenance = event['plan_check']
        assert provenance['repair'] == {'status': 'attempted'}
        assert provenance['objections']
        assert all(item['items'] and item['rule'] for item in provenance['objections'])
        if change == 'unrelated':
            assert any(item['rule'].startswith('guard.') for item in provenance['objections'])


@pytest.mark.parametrize('size,other', [(True, False), (False, True), (True, True)])
@pytest.mark.parametrize('available', [True, False])
def test_t832_every_objection_repairs_or_records_unavailability(planner_request, monkeypatch, size, other, available):
    draft = copy.deepcopy(LEAF)
    planner_request['inputs']['must_split'] = size
    if other:
        draft['lands_on_its_own'] = False
    from planner_fixtures import planner_model
    from tl3_workers import runtime
    monkeypatch.chdir(planner_request['worktree'])
    planner_request['inputs']['repair_plan_consistency'] = available
    model = planner_model(monkeypatch, draft, draft)
    events = []
    runtime.run(planner_request, events.append)
    assert len(model.calls) == (2 if available else 1)
    provenance = events[-1]['plan_check']
    assert provenance['repair'] == ({'status': 'attempted'} if available else
                                      {'status': 'unavailable', 'reason': 'disabled'})
    classes = {item['class'] for item in provenance['objections']}
    assert ('other' in classes) == other
    assert ('size_execution_fit' in classes) == size
    assert any(item['rule'] == ('leaf.not_independent' if other else 'leaf.must_split')
               for item in provenance['objections'])


def test_t832_format_consumes_the_semantic_repair_allowance(planner_request, monkeypatch):
    from planner_fixtures import planner_model
    from tl3_workers import runtime
    planner_request['inputs']['must_split'] = True
    monkeypatch.chdir(planner_request['worktree'])
    planner_request['inputs']['repair_plan_consistency'] = True
    model = planner_model(monkeypatch, {'kind': 'invalid'}, LEAF, CANT_MEET)
    events = []
    runtime.run(planner_request, events.append)
    assert len(model.calls) == 2
    assert events[-1]['plan_check']['repair'] == {'status': 'unavailable', 'reason': 'format_allowance_consumed'}


def test_t832_malformed_semantic_correction_retains_original_objections(planner_request, monkeypatch):
    model, event = _named_run(planner_request, monkeypatch, {**LEAF, 'lands_on_its_own': False}, {'kind': 'invalid'})
    assert len(model.calls) == 2
    assert event['plan_check']['repair'] == {'status': 'attempted'}
    assert any(item['rule'] == 'leaf.not_independent' for item in event['plan_check']['objections'])


@pytest.mark.parametrize('kind', ['regression', 'behavior-preserving'])
@pytest.mark.parametrize('change', ['fixed', 'head_only', 'kind', 'goals', 'files', 'proof', 'forcing', 'cant_meet'])
def test_843_repairs_only_named_base_absent_selections(planner_request, monkeypatch, kind, change):
    from tl3_workers.roles import planner
    from subprocess import CalledProcessError

    base_source = b'def test_api(): assert True\ndef test_existing(): assert True\n'
    missing = 'tests/test_existing.py::test_missing'
    control = 'tests/test_existing.py::test_api'
    draft = copy.deepcopy(LEAF)
    draft['checks'][1]['kind'] = kind
    draft['checks'][1]['node_ids'] = [missing]
    fixed = copy.deepcopy(draft)
    fixed['checks'][1]['node_ids'] = [control]
    if change == 'head_only':
        Path(planner_request['worktree'], 'tests/test_existing.py').write_text(base_source.decode() + 'def test_head_only(): pass\n')
        fixed['checks'][1]['node_ids'] = ['tests/test_existing.py::test_head_only']
    elif change == 'kind':
        fixed['checks'][1]['kind'] = 'regression' if kind == 'behavior-preserving' else 'behavior-preserving'
    elif change == 'goals':
        fixed['checks'][1]['goal_ids'] = [0, 1]
    elif change == 'files':
        fixed['files'].append('tests/test_existing.py')
    elif change == 'proof':
        fixed['checks'][1]['proof'] = {'subjects': [], 'observer_support': [], 'red_message': 'invented'}
    elif change == 'forcing':
        fixed['checks'][1]['forcing'] = {'symptom': 'invented', 'environment': {'X': '1'}, 'patches': []}
    elif change == 'cant_meet':
        fixed = copy.deepcopy(CANT_MEET)
    planner_request['inputs']['base_sha'] = '843-immutable-base'
    reads = []
    def git(root, *args):
        reads.append(args)
        if args[-1] != '843-immutable-base:tests/test_existing.py':
            raise CalledProcessError(128, args)
        return str(len(base_source)).encode() if args[-2] == '-s' else base_source
    monkeypatch.setattr(planner, 'git', git)
    model, event = _named_run(planner_request, monkeypatch, draft, fixed)
    assert len(model.calls) == 2
    assert not model.calls[-1][1]
    assert 'check.base_missing' in model.calls[-1][0][-1].text
    assert missing in model.calls[-1][0][-1].text
    assert reads and all(args[-1].startswith('843-immutable-base:') for args in reads)
    if change == 'fixed':
        assert 'plan_check' not in event
        assert event['reply']['checks'] == fixed['checks']
    else:
        assert event['plan_check']['repair'] == {'status': 'attempted'}
        assert event['plan_check']['objections']
        if change not in {'head_only', 'cant_meet'}:
            assert any(item['rule'].startswith('guard.') for item in event['plan_check']['objections'])


@pytest.mark.parametrize('number', [1455, 2825])
@pytest.mark.parametrize('resolution', ['unchanged', 'cant_meet'])
def test_843_live_golden_gets_one_repair_then_typed_stop(planner_request, monkeypatch, number, resolution):
    import json
    from tl3_workers.roles import planner
    from subprocess import CalledProcessError

    fixture = json.loads(Path(__file__).with_name('fixtures').joinpath(f'planner-843-{number}.json').read_text())
    draft = fixture['draft']
    missing = fixture['objections'][0].split(': ', 1)[1].split('. A new assertion', 1)[0].rstrip('.')
    # The trace preserves complete declarations, but its raw model span is
    # truncated. Base source facts below are explicitly synthetic observations.
    sources = {}
    for check in draft['checks']:
        if check['kind'] in {'regression', 'behavior-preserving'}:
            for node in check['node_ids']:
                if node != missing:
                    path, name = node.split('::')
                    sources[path] = sources.get(path, '') + f'def {name}(): assert True\n'
    sources.setdefault(missing.split('::')[0], '')
    for file in draft['files']:
        path = Path(planner_request['worktree'], file)
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(sources.get(file, '# synthetic source observation\n'))
    def git(root, *args):
        identity = args[-1]
        prefix = fixture['base_sha'] + ':'
        assert identity.startswith(prefix)
        source = sources.get(identity[len(prefix):])
        if source is None:
            raise CalledProcessError(128, args)
        return str(len(source.encode())).encode() if args[-2] == '-s' else source.encode()
    monkeypatch.setattr(planner, 'git', git)
    planner_request['inputs']['base_sha'] = fixture['base_sha']
    planner_request['inputs']['ingest']['goals'] = fixture['goals']
    fixed = draft if resolution == 'unchanged' else copy.deepcopy(CANT_MEET)
    model, event = _named_run(planner_request, monkeypatch, draft, fixed)
    assert len(model.calls) == 2
    assert 'check.base_missing' in model.calls[-1][0][-1].text
    assert missing in model.calls[-1][0][-1].text
    assert event['plan_check']['repair'] == {'status': 'attempted'}
    assert any(item['rule'] == 'check.base_missing' for item in event['plan_check']['objections'])
    assert fixture['objections'][0] in event['reply']['violations']


@pytest.mark.parametrize('red', [1, None, [], {}])
def test_malformed_proof_omission_keeps_the_single_format_repair(planner_request, monkeypatch, red):
    draft = copy.deepcopy(LEAF)
    draft['checks'][0]['proof'] = {'red_message': red}
    model, event = _named_run(planner_request, monkeypatch, draft, LEAF)
    assert len(model.calls) == 2
    assert not model.calls[-1][1]
    assert event['reply']['violations'] == []
    assert 'plan_check' not in event


def _fixture_1574():
    import json
    return json.loads(Path(__file__).with_name('fixtures').joinpath('planner-1574.json').read_text())


def test_1574_repair_line_asks_only_for_the_empty_contract_the_guard_accepts():
    from tl3_workers.evidence import preflight
    from tl3_workers.roles import planner
    from tl3_workers.rules import permits

    fixture = _fixture_1574()
    draft = copy.deepcopy(fixture['draft'])
    objections = []
    preflight(fixture['inputs'], draft, planning=True, objections=objections)
    error = planner.PlanConsistencyError(fixture['draft'], draft, objections, fixture['inputs'])
    assert '/evidence_contract' in error.repair_text
    assert 'const []' in error.repair_text
    assert permits(error.rule_violations, fixture['draft'], {**fixture['draft'], 'evidence_contract': []}, role='planner')
    assert error.check({**fixture['draft'], 'evidence_contract': fixture['correction_evidence_contract']})


@pytest.mark.parametrize('case', ['proof.omitted', 'split.forecast_fits', 'check.tests_empty'])
def test_a1_empty_allowed_set_is_an_explicit_terminal_row(planner_request, monkeypatch, case):
    """PQ #2648 twice, and the TESTS_FILL fallback, must not promise impossible edits."""
    from tl3_workers.roles import planner
    from tl3_workers.rules import CantMeet, permits

    monkeypatch.chdir(planner_request['worktree'])
    inputs = planner_request['inputs']
    inputs['repair_plan_consistency'] = True
    monkeypatch.setattr(planner.Forecaster, 'measure', lambda *args: {'fits': True, 'tokens': 97674})
    monkeypatch.setattr(planner, 'execution_forecast', lambda *args, **kwargs: {
        'fits': True, 'exploration_declared': True, 'estimated_tool_turns': 10,
        'allowed_tool_turns': 160, 'components': {}})
    draft = copy.deepcopy(SPLIT if case.startswith('split.') else LEAF)
    if case == 'proof.omitted':
        draft['checks'][0].pop('proof')
    elif case == 'check.tests_empty':
        draft['checks'][0]['node_ids'] = []
    with pytest.raises(planner.PlanConsistencyError) as caught:
        planner.check_reply(inputs, draft)
    error = caught.value
    assert 'cant_meet' in error.repair_text
    assert 'State the expected assertion message' not in error.repair_text
    assert 'Replace only the invalid split reason' not in error.repair_text
    rows = getattr(error, 'rule_violations', ())
    matching = [item for item in rows if item.rule.id == case]
    assert matching, 'every refusal must be a row, including fallback footprints'
    assert any(isinstance(item.rule.repair(item.facts), CantMeet) for item in matching)
    assert permits(rows, error.raw_draft, CANT_MEET, role='planner')


def _row_case(rule_id):
    from tl3_harness.model import named
    from tl3_workers.roles.planner_rules import inventory

    footprint = inventory()[rule_id]['footprint']
    proof = {'subjects': [], 'observer_support': [], 'red_message': 'expected regression'}
    check = {'kind': 'demonstrating', 'node_ids': ['test.py::test_old'], 'goal_ids': [0], 'proof': proof}
    draft = {'kind': 'leaf', 'files': ['pkg.py', 'test.py'], 'checks': [check],
             'evidence_contract': [], 'estimated_exploration_turns': 0}
    inputs = {'must_split': False, 'ingest': {'goals': [{'description': 'The obligation.',
        'proof_obligation': {'subjects': ['pkg.py::target'], 'quote': 'expected regression'}}]}}
    items, witness = {}, None
    completed = {'forecast': {'fits': True}, 'execution': {'fits': True}}
    if footprint == 'CODE':
        draft['execution'] = {}
        items = {'keys': ['execution']}
    elif footprint == 'EVIDENCE_EMPTY':
        del draft['evidence_contract']
        items, witness = {'field': 'evidence_contract'}, []
    elif footprint == 'PROOF_FILL':
        del check['proof']
        items, witness = {'check': 0}, proof
    elif footprint == 'RED_FILL':
        proof['red_message'] = ''
        items, witness = {'check': 0, 'obligation': {'quote': 'expected regression'}}, 'expected regression'
    elif footprint == 'SUBJECT_ADD':
        items, witness = {'check': 0, 'subjects': ['pkg.py::target']}, ['pkg.py::target']
    elif footprint in {'EXACT_SELECT', 'BASE_SELECT', 'FORCED_SELECT'}:
        items, witness = {'check': 0, 'nodes': ['test.py::test_old']}, 'test.py::test_new'
    elif footprint in {'FILES_FILL', 'PROOF_FILE', 'EDIT_FILE', 'FILE_PATH'}:
        draft['files'] = [] if footprint == 'FILES_FILL' else ['test.py']
        if footprint == 'FILE_PATH':
            draft['files'] = ['./pkg.py']
        items = {'path': './pkg.py' if footprint == 'FILE_PATH' else 'pkg.py',
                 'unit': 'pkg.py::target', 'node': 'test.py::test_old'}
        witness = ['pkg.py'] if footprint in {'FILES_FILL', 'FILE_PATH'} else ['test.py', 'pkg.py']
    elif footprint in {'COVERAGE', 'GOAL_ADD', 'GOAL_DROP', 'GOAL_SANITIZE'}:
        check['goal_ids'] = [9, 0, 0] if footprint in {'GOAL_DROP', 'GOAL_SANITIZE'} else []
        items, witness = {'check': 0, 'goals': [0], 'unknown': [9]}, [0]
        proof['subjects'] = ['pkg.py::target']
    elif footprint in {'PIECE_GOALS', 'PIECE_DROP', 'PIECE_COVER'}:
        draft = {'kind': 'split', 'reason': 'multiple_repos', 'pieces': [
            {'brief': 'The obligation.', 'files': ['pkg.py'], 'goal_ids': [9] if footprint == 'PIECE_DROP' else []}]}
        items, witness = {'piece': 0, 'missing': [0]}, [] if footprint == 'PIECE_DROP' else [0]
    elif footprint == 'SPLIT_REASON':
        draft = {'kind': 'split', 'reason': 'does_not_fit', 'pieces': []}
        inputs['must_split'] = True
        items, witness = {'reason': 'does_not_fit'}, 'out_of_room'
    elif footprint == 'SPLIT':
        inputs['must_split'] = True
        items = {'must_split': True}
        witness = {'kind': 'split', 'reason': 'out_of_room', 'evidence_contract': [], 'estimated_exploration_turns': 0,
                   'pieces': [{'brief': 'The obligation.', 'files': ['pkg.py', 'test.py'], 'goal_ids': [0],
                               'estimated_exploration_turns': 0},
                              {'brief': 'Control.', 'files': ['test.py'], 'goal_ids': [0], 'estimated_exploration_turns': 0}]}
    elif footprint == 'PROOF_DEDUP':
        proof['subjects'] = ['pkg.py', 'pkg.py']
        items, witness = {'check': 0}, {**proof, 'subjects': ['pkg.py']}
    elif footprint == 'NODE_DEDUP':
        check['node_ids'] *= 2
        items, witness = {'check': 0}, ['test.py::test_old']
    elif footprint == 'EDIT_DROP':
        draft['edits_tests'] = ['test.py::test_old']
        items, witness = {'node': 'test.py::test_old'}, []
    elif footprint == 'EDIT_COVER':
        items, witness = {'nodes': ['test.py::test_new']}, ['test.py::test_old', 'test.py::test_new']
    elif footprint == 'FROZEN':
        check['kind'] = 'regression'
        check['node_ids'].append('test.py::test_control')
        items, witness = {'node': 'test.py::test_old'}, ['test.py::test_control']
    elif footprint == 'MEMORY_DROP':
        draft['memory_ids'] = ['unknown']
        items, witness = {'ids': ['unknown']}, []
    elif footprint in {'FLAG_DROP', 'FLAG_CLAIM'}:
        draft['memory_flags'] = [{'id': 'm', 'claim': 'unsupported'}]
        items, witness = {'ids': ['m'], 'flag': 0}, []
    elif footprint == 'SYMPTOM_FIELDS':
        check['declared_symptom'] = {'exception': 'builtins.ValueError', 'phase': 'call', 'extra': True}
        items, witness = {'check': 0}, {'exception': 'builtins.ValueError', 'phase': 'call', 'node': 'test.py::test_old'}
    elif footprint in {'PATCH_MODE', 'FORCING_KEYS'}:
        check['forcing'] = {'symptom': 'x', 'environment': {}, 'patches': [{'target': 'pkg.VALUE', 'value': 1}]}
        witness = copy.deepcopy(check['forcing'])
        if footprint == 'PATCH_MODE':
            check['forcing']['patches'][0]['extra'] = True
        else:
            check['forcing']['extra'] = True
        items = {'check': 0}
    elif footprint == 'METHOD_REPLACE':
        capability = {'tool': 'run_tests', 'executor': 'fleet', 'runner_features': [], 'artifacts': [], 'methods': [], 'protected_files': []}
        requirement = {**capability, 'tool': 'absent', 'provenance': {'kind': 'method'}}
        draft['evidence_contract'] = [requirement]
        inputs['tool_capabilities'] = [capability]
        items, witness = {'requirement': requirement}, {**requirement, **capability}
    facts = {'reply': draft, 'draft': copy.deepcopy(draft), 'completed': completed, 'inputs': inputs,
             'diagnostics': [named(rule_id, '', items)], 'base_nodes': {'test.py::test_old'},
             'base_sources': {'test.py': 'def test_new(): assert True\n'}}
    return facts, witness


from tl3_workers.roles.planner_rules import inventory as _rule_inventory


@pytest.mark.parametrize('rule_id', sorted(_rule_inventory()))
def test_a1_each_registered_rule_renders_and_enforces_its_repair(rule_id):
    """PRD #878: every rule and all 43 footprints have an executable interface contract."""
    from dataclasses import replace
    from tl3_workers.roles.planner_rules import rows
    from tl3_workers.rules import CantMeet, apply_edit, evaluate, permits, register

    row = next(row for row in rows() if row.id == rule_id)
    facts, witness = _row_case(rule_id)
    found = evaluate((row,), facts, role='planner')
    assert found, rule_id
    terminal = any(isinstance(row.repair(item.facts), CantMeet) for item in found)
    if terminal:
        witness = copy.deepcopy(CANT_MEET)
    registered = register(replace(row, example=facts, witness=witness), roles={'planner'})
    after = copy.deepcopy(facts['reply'])
    for item in found:
        rendered = item.render(role='planner')
        assert row.id in rendered and row.names(item.facts)[0].render() in rendered
        assert row.allowed(item.facts).render() in rendered
        repair = row.repair(item.facts)
        after = witness if terminal else apply_edit(after, repair, witness)
    assert permits(found, facts['reply'], after, role='planner')
    assert not evaluate((registered,), {**facts, 'reply': after}, role='planner')
    if not terminal:
        changed = {**after, 'unrelated': True}
        assert not permits(found, facts['reply'], changed, role='planner')


def test_a1_reports_all_rows_without_clipping():
    from tl3_harness.model import named
    from tl3_workers.roles.planner_rules import violations
    from tl3_workers.rules import render_all

    facts = {'reply': {'kind': 'leaf'}, 'draft': {'kind': 'leaf'}, 'completed': {},
             'inputs': {'ingest': {'goals': []}},
             'diagnostics': [named('leaf.design_open', '', {'field': 'x' * 2500}),
                             named('leaf.not_independent', '', {'field': 'lands_on_its_own'})]}
    rendered = render_all(violations(facts), role='planner')
    assert 'x' * 2500 in rendered and 'leaf.not_independent' in rendered
    assert ' [...]' not in rendered


def test_839_closed_prerequisite_input_validates(planner_request):
    from tl3_workers.contracts import validate
    inputs = copy.deepcopy(planner_request['inputs'])
    inputs['prerequisite_closures'] = [{'kind': 'issue', 'number': 2763, 'reason': 'not_planned'}]
    validate('planner-input', inputs)
    inputs['prerequisite_closures'][0]['number'] = 0
    from jsonschema import ValidationError
    with pytest.raises(ValidationError):
        validate('planner-input', inputs)
