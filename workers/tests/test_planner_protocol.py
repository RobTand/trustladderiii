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
    assert 'check-1' in history[-1].text and 'proof.red_message' in history[-1].text
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


@pytest.mark.parametrize('target', [2047, 2048, 2049])
@pytest.mark.parametrize('unicode', [False, True])
def test_named_repair_message_preserves_all_identity_bytes(target, unicode):
    from tl3_harness.model import named
    from tl3_workers.roles.planner import message

    def objection(path):
        return named('leaf.token_fit', '', {'path': path})
    fixed = len(message([objection('')]).encode())
    prefix = 'é界' if unicode else ''
    identity = prefix + 'x' * (target - fixed - len(prefix.encode()))
    result = message([objection(identity)])
    if target == 2049:
        assert result is None
    else:
        assert len(result.encode()) == target
        assert identity in result


def test_named_repair_message_clips_only_optional_descriptions():
    from tl3_harness.model import named
    from tl3_workers.roles.planner import message

    objection = named('goal.coverage', 'full safe diagnostic', {
        'goals': [7, 9], 'descriptions': {7: 'é' * 3000, 9: '界' * 3000}})
    result = message([objection])
    assert len(result.encode()) <= 2048
    assert 'goal 7:' in result and 'goal 9:' in result
    assert abs(result.count('é') * 2 - result.count('界') * 3) <= 2
    assert ' [...]' in result


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
    """tessera #1574: the one EVIDENCE_EMPTY repair line also told the model to write requirements."""
    from tl3_workers.evidence import preflight
    from tl3_workers.roles import planner

    fixture = _fixture_1574()
    draft = copy.deepcopy(fixture['draft'])
    objections = []
    preflight(fixture['inputs'], draft, planning=True, objections=objections)
    assert [(item.rule, item.items) for item in objections] == [
        (item['rule'], item['items']) for item in fixture['objections']]
    line = planner.message(objections)
    assert line.split('\n')[1].startswith('evidence.contract_missing field="evidence_contract": ')
    assert 'evidence_contract: []' in line
    assert 'Return evidence_contract with source requirements' not in line, line
    assert 'Code supplies the inherited source requirements' in line, line
    # Production assembly: the same line reaches the model as the semantic repair text.
    error = planner.PlanConsistencyError.semantic(fixture['draft'], draft, objections, fixture['inputs'],
                                                  planner.PlanConsistencyError([], draft=fixture['draft']))
    assert error.repair_text == line
    # The guard is unchanged: the answer the line asks for passes; the live correction does not.
    contract = error.contract
    assert contract.check({**fixture['draft'], 'evidence_contract': []}) == []
    rejected = contract.check({**fixture['draft'], 'evidence_contract': fixture['correction_evidence_contract']})
    assert [(item.rule, item.items) for item in rejected] == [('guard.fields_changed', {'paths': ['/evidence_contract']})]


# Per footprint: words naming the one edit its Correction guard accepts. A repair
# line must carry them. Footprints with no CONSTRAINTS entry have no guard
# permission, so their only accepted reply is cant_meet.
REPAIR_INTENT = {
    'NONE': 'return cant_meet', 'EDIT_SELECT': 'return cant_meet', 'FILE_DROP': 'return cant_meet',
    'FORCING_RESERVED': 'return cant_meet', 'FORCING_RULE': 'return cant_meet', 'FORCING_TEXT': 'return cant_meet',
    'OBSERVER_SELECT': 'return cant_meet', 'PROTECTED': 'return cant_meet', 'RECEIPT_SELECT': 'return cant_meet',
    'TESTS_FILL': 'return cant_meet',
    'CODE': 'Delete only named code-owned keys', 'COVERAGE': 'Only add this citation to existing checks',
    'RED_FILL': 'Fill only blank proof.red_message', 'PROOF_FILL': 'Fill only absent proof keys',
    'SUBJECT_ADD': 'Append only named missing subjects', 'EXACT_SELECT': 'Replace only named inexact selectors',
    'BASE_SELECT': 'Replace only named absent selectors with exact base controls',
    'FORCED_SELECT': 'Replace only named absent forced selectors', 'EDIT_COVER': 'Cover only those edits',
    'SPLIT': 'Preserve every original obligation', 'FILES_FILL': 'Fill empty files only',
    'GOAL_ADD': 'Fill empty citations only', 'GOAL_DROP': 'Remove only named unknown goal ids',
    'GOAL_SANITIZE': 'Remove only invalid or duplicate citations', 'PIECE_GOALS': 'Fill only empty citations',
    'PIECE_DROP': 'Remove only unknown piece goal ids', 'PIECE_COVER': 'Add only missing goal ids',
    'PROOF_FILE': 'Add only the named source-required path', 'EDIT_FILE': 'Add only the named file',
    'EDIT_DROP': 'Drop only the named absent-base edit', 'FROZEN': 'Remove only named edited nodes from frozen checks',
    'EVIDENCE_EMPTY': 'evidence_contract: []', 'METHOD_REPLACE': 'Replace only named method choices',
    'MEMORY_DROP': 'Remove only named unsupplied memory ids', 'FLAG_DROP': 'Remove only named uncited flags',
    'FLAG_CLAIM': 'Drop the named flag or replace only its claim', 'SYMPTOM_FIELDS': 'Remove extra symptom keys',
    'PATCH_MODE': 'Remove extra patch keys', 'FORCING_KEYS': 'Remove only extra top-level forcing keys',
    'PROOF_DEDUP': 'Remove only identical repeated proof units', 'NODE_DEDUP': 'Remove only duplicate selectors',
    'FILE_PATH': 'unique confined relative path', 'SPLIT_REASON': 'Replace only the invalid split reason',
}
ANY = None
# Every edit a diagnostic asks for, and the footprints whose guard accepts it.
# A new asking diagnostic fails the table test until it is classified here.
DIAGNOSTIC_ASKS = {
    'Return evidence_contract with source requirements': set(),  # code owns inherited requirements
    'Leave it out': set(),  # no leaf guard removes a declared file
    'reduce the selected tests': set(),  # SPLIT keeps every check; only FROZEN drops named edited nodes
    'Use a relative worktree path': {'FILE_PATH'},
    'Replace `': {'EXACT_SELECT'},
    'Name it in a demonstrating check': {'EDIT_COVER'},
    'Name it only in a demonstrating check': {'FROZEN'},
    'Preserve every obligation in bounded pieces': {'SPLIT'},
    'Propose smaller pieces': {'SPLIT'},
    'omit execution from the model reply': {'CODE'},
    'State the expected assertion message': {'PROOF_FILL'},
    'Only add the named subjects': {'SUBJECT_ADD'},
    'Preserve': ANY,  # freezing is accepted under every footprint
    # Not a plan edit: only the operator can change request scope, so cant_meet
    # (the NONE line) is the planner's one honest reply.
    'Change the request scope or repository convention': {'NONE'},
}
ASKING = r'(?:[Oo]nly )?(?:Return|Add|Leave|Replace|Remove|Drop|Name|State|Propose|Preserve|Change|Fill|Delete|Use|[Rr]educe|[Oo]mit|Keep|List|Append|Cover|Resume)\b'


def _unpack_corpus(value):
    from tl3_harness.model import named
    if isinstance(value, list):
        return [_unpack_corpus(item) for item in value]
    if not isinstance(value, dict):
        return value
    if '$diagnostic' in value:
        text, rule, items = value['$diagnostic']
        return named(rule, text, _unpack_corpus(items))
    if '$dict' in value:
        return {_unpack_corpus(key): _unpack_corpus(item) for key, item in value['$dict']}
    if '$set' in value or '$tuple' in value:
        return [_unpack_corpus(item) for item in value.get('$set', value.get('$tuple'))]
    if '$bytes_utf8' in value:
        return value['$bytes_utf8']
    return {key: _unpack_corpus(item) for key, item in value.items()}


def _emitted_diagnostics(planner_request, monkeypatch):
    """Real diagnostics: the P2a corpus records, plus emitters it lacks."""
    import gzip
    import json
    from tl3_harness import Check, Kind
    from tl3_harness.limits import validate_check_plan, validate_plan_size
    from tl3_harness.model import Diagnostic, named
    from tl3_workers import forecast
    from tl3_workers.evidence import EvidenceContractError
    from tl3_workers.roles import planner

    found = []

    def walk(value):
        if isinstance(value, dict):
            if '$diagnostic' in value:
                found.append(_unpack_corpus(value))
            for item in value.values():
                walk(item)
        elif isinstance(value, list):
            for item in value:
                walk(item)
    walk(json.loads(gzip.decompress(Path(__file__).with_name('planner-vectors.json.gz').read_bytes())))
    found.append(EvidenceContractError().args[0])
    for limit in (lambda: validate_plan_size(('tests/test_a.py::' + 'a' * 4_200_000,)),
                  lambda: validate_check_plan((Check('check-1', Kind.REGRESSION,
                                                     tuple(f'tests/test_a.py::test_{i}' for i in range(700))),))):
        with pytest.raises(ValueError) as error:
            limit()
        found.append(planner.contextual(error.value, checks=[]))
    # Every forecaster problem reaches the plan through check_reply's own wrapper.
    problems = [forecast.DeclaredFileError('greeting.py', 'unusable')] + [
        forecast.DeclaredFileError('greeting.py', named(rule, rule, {'path': 'greeting.py'}), 7)
        for rule in ('path.not_regular', 'path.binary', 'path.encoding', 'path.hard_cap', 'path.noncanonical')]

    # Budget arithmetic is Rust-owned and out of scope here; fixed facts keep the
    # wrapper reachable without a checker call, as the P2a recorder does.
    class Problems:
        def __init__(self, config):
            self.problems = list(problems)

        def measure(self, *args):
            return {'fits': True, 'tokens': 1}
    monkeypatch.setattr(planner, 'Forecaster', Problems)
    monkeypatch.setattr(planner, 'execution_forecast',
                        lambda *args, **kwargs: {'exploration_declared': False, 'fits': True, 'estimated_tool_turns': 1})
    monkeypatch.chdir(planner_request['worktree'])
    reply = copy.deepcopy(LEAF)
    planner.check_reply(planner_request['inputs'], reply)
    found.extend(item for item in reply['violations'] if isinstance(item, Diagnostic) and item.rule.startswith('path.'))
    by_rule = {}
    for item in found:
        if isinstance(item, Diagnostic) and item.rule in planner.rules():
            by_rule.setdefault(item.rule, {})[json.dumps([str(item), planner.identities(item.items)])] = item
    return {rule: list(items.values()) for rule, items in by_rule.items()}


def test_every_footprint_repair_line_asks_only_for_what_its_guard_accepts(planner_request, monkeypatch):
    """#1574 unactionable-feedback guard: one table over every footprint in plan_rules.json."""
    import re
    from tl3_harness.model import named
    from tl3_workers.roles import planner

    footprints = {}
    for rule, entry in planner.rules().items():
        footprints.setdefault(entry['footprint'], []).append(rule)
    assert set(REPAIR_INTENT) == set(footprints)
    samples = _emitted_diagnostics(planner_request, monkeypatch)
    for rule in ('evidence.contract_missing', 'size.report', 'size.result', 'path.binary', 'path.hard_cap',
                 'path.noncanonical', 'path.unusable', 'check.selector_inexact', 'edit.uncovered'):
        assert samples.get(rule), rule
    failures = []
    for footprint, members in sorted(footprints.items()):
        intent = REPAIR_INTENT[footprint]
        for rule in members:
            for diagnostic in samples.get(rule, [named(rule, '', {})]):
                line = planner.message([diagnostic])
                if line is None:
                    continue
                body = line.split('\n', 1)[1]
                if intent not in body:
                    failures.append(f'{footprint} {rule}: line lacks the accepted edit {intent!r}: {body}')
                if 'description' in diagnostic.items or 'descriptions' in diagnostic.items:
                    continue  # the line quotes a source goal, not a diagnostic
                constraint = planner.CONSTRAINTS.get(footprint, planner.CONSTRAINTS['NONE'])
                asked = body.split(constraint, 1)[-1] if rule != 'goal.coverage' else ''
                for clause in re.split(r'(?<=[.;:])\s+', asked):
                    if not re.match(ASKING, clause.strip()):
                        continue
                    kind = next((ask for ask in DIAGNOSTIC_ASKS if clause.strip().startswith(ask)), None)
                    if kind is None:
                        failures.append(f'{footprint} {rule}: unclassified ask {clause!r}')
                    elif DIAGNOSTIC_ASKS[kind] is not ANY and footprint not in DIAGNOSTIC_ASKS[kind]:
                        failures.append(f'{footprint} {rule}: line also asks {clause.strip()!r}, '
                                        f'which the {footprint} guard rejects; it accepts only {intent!r}')
    assert not failures, '\n'.join(failures)
