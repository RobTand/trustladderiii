"""#899 review regression coverage at the assembly and result boundaries."""
import json
from pathlib import Path
from types import SimpleNamespace

import pytest

from proof_row_cases import partitions
from tl3_harness import model, proof
from tl3_harness.proof_rules import ROWS


def assemble(case, root):
    for side in ('before', 'after'):
        for name, source in case[side].items():
            path = root / side / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(source)
    destination = root / 'staged'
    if case.get('destination_exists'):
        destination.mkdir(parents=True)
    # Module selections exercise the assembler defense below model validation.
    factory = model.Check if all('::' in node for node in case['tests']) else SimpleNamespace
    check = factory(name='row witness', kind=model.Kind.DEMONSTRATING, tests=tuple(case['tests']),
                    proof=model.Proof.parse(case['proof']), forcing=None, declared_symptom=None)
    return proof.assemble_proof(root / 'before', root / 'after', destination, check, case['config'])


@pytest.mark.parametrize('row_id', [id for id in ROWS if id not in ('forcing.raise_class', 'proof.staging.single_check')])
def test_row_has_real_assembly_input_and_source_repair(tmp_path, row_id):
    from tl3_harness.rules import CantMeet, Edit, SourceEdit
    case = partitions()[row_id]
    with pytest.raises(model.ProofRefusal) as caught:
        assemble(case['bad'], tmp_path / 'bad')
    assert any(v.rule.id == row_id for v in caught.value.violations)
    assert caught.value.rule == row_id
    for v in caught.value.violations:
        message = v.render(role='maker')
        assert v.rule.id in message and 'Allowed:' in message
        subjects = v.rule.names(v.facts)
        assert subjects and all(name.render() in message for name in subjects)
        if isinstance(v.repair, CantMeet):
            assert v.repair.reason in message
        elif isinstance(v.repair, SourceEdit):
            assert v.repair.instruction in message
        else:
            assert isinstance(v.repair, Edit) and v.repair.pointer.render() in message
    assemble(case['good'], tmp_path / 'good')


def test_recorded_main_verdict_table_exists_and_covers_every_row():
    path = Path(__file__).parent / 'fixtures/subject_observer/proof_verdicts.json'
    assert path.is_file(), 'Generate the recorded verdict table on BuildBuddy from origin/main'
    table = json.loads(path.read_text())
    assert len(table['source_sha']) == 40
    covered = {entry['expected'].get('rule') for entry in table['cases'].values()}
    assert set(ROWS) <= covered | {'forcing.raise_class'}


def test_non_definition_names_are_bad_values():
    with pytest.raises(model.ProofRefusal) as caught:
        proof.validate_support('def helper(value): return sorted(value)', 'tests/test_observer.py')
    assert caught.value.rule == 'proof.support.call_allowlist'
    assert all(name.kind != 'symbol' or name.value.endswith('::helper')
               for violation in caught.value.violations for name in violation.rule.names(violation.facts))
    assert 'literal arguments' in str(caught.value)


def test_direct_forcing_refusal_precedes_a_later_invalid_json_patch():
    with pytest.raises(model.ProofRefusal) as caught:
        model.Forcing('flake', patches=(('app.first', 'raises', '"AssertionError"'), ('app.second', 'return', 'bad json')))
    assert caught.value.rule == 'forcing.raise_class'
    assert str(caught.value).count('ArithmeticError') == 1


def test_test_path_reports_the_configured_patterns(tmp_path):
    with pytest.raises(model.ProofRefusal) as caught:
        assemble(partitions()['proof.observer.test_path']['bad'], tmp_path)
    assert caught.value.rule == 'proof.observer.test_path'
    assert 'specs/*.py' in str(caught.value)


def test_syntax_error_names_the_parsed_support_file(tmp_path):
    case = partitions()['proof.source.unparsable']['bad']
    case['proof']['observer_support'] = ['support.py::helper']
    case['after']['support.py'] = 'def helper(:\n'
    with pytest.raises(model.ProofRefusal) as caught:
        assemble(case, tmp_path)
    assert caught.value.rule == 'proof.source.unparsable'
    assert caught.value.module == 'support.py'


def test_export_has_one_path_template_and_a_regeneration_command():
    from tl3_harness.proof_rules import export_rows
    rows = export_rows()
    loader = next(row for row in rows if row['id'] == 'proof.loader.stdlib_shadow')
    paths = [name for name in loader['names'] if name['kind'] == 'path']
    assert len({name['value'] for name in paths}) == len(paths)
    assert 'python -m tl3_harness.proof_rules' in (Path(__file__).parents[1] / 'README.md').read_text()


def recorded_table():
    return json.loads((Path(__file__).parent / 'fixtures/subject_observer/proof_verdicts.json').read_text())


@pytest.mark.parametrize('key,record', [(key, item) for key, item in recorded_table()['cases'].items()
                                     if item['input']['function'] != 'staging'], ids=lambda value: value if isinstance(value, str) else '')
def test_recorded_main_verdict(tmp_path, key, record):
    import base64
    import hashlib
    data = record['input']
    if data['function'] == 'assemble_proof':
        for side in ('before', 'after'):
            (tmp_path / side).mkdir()
            for name, content in data[side].items():
                path = tmp_path / side / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(base64.b64decode(content))
                path.chmod(data['modes'][side][name])
        staged = tmp_path / 'staged'
        if data['destination_exists']:
            staged.mkdir()
        declaration = data['check']
        factory = model.Check if all('::' in node for node in declaration['tests']) else SimpleNamespace
        check = factory(name=declaration['name'], kind=model.Kind(declaration['kind']), tests=tuple(declaration['tests']),
                        proof=model.Proof.parse(declaration['proof']) if declaration['proof'] else None,
                        forcing=model.Forcing.parse(declaration['forcing']) if declaration['forcing'] else None,
                        declared_symptom=declaration['declared_symptom'])
        def run():
            return proof.assemble_proof(tmp_path / 'before', tmp_path / 'after', staged, check, data['config'])
    else:
        def run():
            return proof.validate_support(*data['args'], **data['kwargs'])
    if 'rule' in record['expected']:
        with pytest.raises(model.ProofRefusal) as caught:
            run()
        assert caught.value.rule == record['expected']['rule'], key
    else:
        receipt = run()
        actual = hashlib.sha256(json.dumps(receipt, sort_keys=True, separators=(',', ':')).encode()).hexdigest()
        assert actual == record['expected']['accepted']['receipt_sha256'], key
        if data['function'] == 'assemble_proof':
            assert proof.tree_sha256(staged) == record['expected']['accepted']['staged_tree_sha256'], key


def test_role_repairs_and_absolute_declaration_pointers(tmp_path):
    from tl3_harness.rules import Edit, SourceEdit, apply_edit, permits
    case = partitions()['proof.observer.exact_method']['bad']
    for side in ('before', 'after'):
        for name, text in case[side].items():
            path = tmp_path / side / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(text)
    check = model.Check('repair', model.Kind.DEMONSTRATING, tuple(case['tests']), proof=model.Proof.parse(case['proof']))
    with pytest.raises(model.ProofRefusal) as caught:
        proof.assemble_proof(tmp_path / 'before', tmp_path / 'after', tmp_path / 'planner', check, case['config'], role='planner', check_index=3)
    assert caught.value.rule == 'proof.observer.exact_method'
    violation = caught.value.violations[0]
    assert isinstance(violation.repair, Edit)
    assert violation.repair.pointer.value == '/checks/3/tests/0'
    assert violation.allowed.value == ['tests/test_observer.py::TestOld::test_observer']
    assert len(violation.allowed.value) <= 10
    reply = apply_edit(violation.facts['reply'], violation.repair, violation.allowed.value[0])
    assert permits(violation, violation.facts['reply'], reply, role='planner')
    repaired = model.Check('repair', model.Kind.DEMONSTRATING, tuple(reply['checks'][3]['tests']), proof=check.proof)
    proof.assemble_proof(tmp_path / 'before', tmp_path / 'after', tmp_path / 'repaired', repaired, case['config'],
                         role='planner', check_index=3)
    case = partitions()['proof.support.call_allowlist']['bad']
    for side in ('before', 'after'):
        for name, text in case[side].items():
            (tmp_path / side / name).write_text(text)
    check = model.Check('repair', model.Kind.DEMONSTRATING, tuple(case['tests']), proof=model.Proof.parse(case['proof']))
    with pytest.raises(model.ProofRefusal) as caught:
        proof.assemble_proof(tmp_path / 'before', tmp_path / 'after', tmp_path / 'maker', check, case['config'],
                             footprint=('tests/test_observer.py',), check_index=4)
    assert caught.value.rule == 'proof.support.call_allowlist'
    assert isinstance(caught.value.violations[0].repair, SourceEdit)
    assert 'cant_meet' not in str(caught.value)
    assert caught.value.violations[0].facts['field'] == '/checks/4/proof/observer_support/0'
    assert caught.value.as_json()[0]['role'] == 'maker'


def test_forcing_pointer_uses_its_plan_index():
    with pytest.raises(model.ProofRefusal) as caught:
        model.Forcing.parse({'symptom': 'flake', 'patches': [{'target': 'app.value', 'raises': 'AssertionError'}]}, check_index=3)
    assert caught.value.rule == 'forcing.raise_class'
    assert caught.value.violations[0].repair.pointer.value == '/checks/3/forcing/patches/0/raises'


@pytest.mark.parametrize('source,why', [
    ('len = lambda value: 1\ndef helper(): return len(())', 'shadowed in this module'),
    ('def helper(value): return sorted(value)', 'literal arguments and no keywords'),
    ('def helper(): return sorted((), reverse=True)', 'literal arguments and no keywords'),
    ('def helper(value): return hasattr(value, "x")', 'can run code from other objects'),
    ('def helper(value): return value.method()', 'calls a method on another object'),
    ('def helper(value): return value()()', 'calls the result of another expression'),
])
def test_callee_explanations_are_owned_by_the_row(source, why):
    with pytest.raises(model.ProofRefusal) as caught:
        proof.validate_support(source, 'tests/test_observer.py')
    assert any(v.rule.id == 'proof.support.call_allowlist' and why in v.render(role='maker') for v in caught.value.violations)


def test_internal_destination_and_uneditable_syntax_are_terminal(tmp_path):
    from tl3_harness.rules import CantMeet
    with pytest.raises(model.ProofRefusal) as caught:
        assemble(partitions()['proof.tree.destination_fresh']['bad'], tmp_path / 'destination')
    assert caught.value.rule == 'proof.tree.destination_fresh'
    assert caught.value.violations[0].rule.gating
    assert isinstance(caught.value.violations[0].repair, CantMeet)
    assert 'TL3-side assembly fault' in str(caught.value)


def test_complete_proof_messages_still_obey_the_result_byte_limit():
    from tl3_harness import execution
    from tl3_harness.limits import MAX_RESULT_BYTES
    check = model.Check('bounded rows', model.Kind.DEMONSTRATING, ('tests/test_observer.py::test_observer',))
    error = model.ProofRefusal('proof.support.call_allowlist', module='tests/test_observer.py', why='x' * MAX_RESULT_BYTES)
    with pytest.raises(model.HarnessInputError, match='result byte budget'):
        execution.refused((check,), error)


@pytest.mark.parametrize('role,editable,repair_kind', [
    ('planner', True, 'CantMeet'), ('maker', False, 'CantMeet'), ('maker', True, 'SourceEdit'),
])
def test_syntax_repairs_respect_role_and_footprint(tmp_path, role, editable, repair_kind):
    from tl3_harness.rules import CantMeet, SourceEdit
    case = partitions()['proof.source.unparsable']['bad']
    for side in ('before', 'after'):
        for name, text in case[side].items():
            path = tmp_path / side / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(text)
    check = model.Check('syntax repair', model.Kind.DEMONSTRATING, tuple(case['tests']), proof=model.Proof.parse(case['proof']))
    with pytest.raises(model.ProofRefusal) as caught:
        proof.assemble_proof(tmp_path / 'before', tmp_path / 'after', tmp_path / 'staged', check, case['config'],
                             role=role, footprint=('tests/test_observer.py',) if editable else ())
    assert caught.value.rule == 'proof.source.unparsable'
    repair = caught.value.violations[0].repair
    assert isinstance(repair, {'CantMeet': CantMeet, 'SourceEdit': SourceEdit}[repair_kind])
    if isinstance(repair, SourceEdit):
        assert repair.path == 'tests/test_observer.py'
        assert 'fix the syntax error at line ' in repair.instruction
        assert 'cant_meet' not in str(caught.value)
    else:
        assert 'cant_meet' in str(caught.value)



def planner_refusal(case, root, *, monkeypatch=None, probes=None):
    for side in ('before', 'after'):
        for name, source in case[side].items():
            path = root / side / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(source)
    check = model.Check('selection', model.Kind.DEMONSTRATING, tuple(case['tests']), proof=model.Proof.parse(case['proof']))
    if probes is not None:
        original = proof._assemble_proof
        def counted(*args, **kwargs):
            probes.append(args[3])
            return original(*args, **kwargs)
        monkeypatch.setattr(proof, '_assemble_proof', counted)
    with pytest.raises(model.ProofRefusal) as caught:
        proof.assemble_proof(root / 'before', root / 'after', root / 'stage', check, case['config'], role='planner', check_index=3)
    return caught.value


def test_planner_support_replacements_skip_duplicates_and_keep_the_list(tmp_path):
    from tl3_harness.rules import Edit
    case = partitions()['proof.support.call_allowlist']['bad']
    module = 'tests/test_observer.py'
    case['after'][module] = ('def test_control(): pass\ndef helper_bad(value=1): return sorted(value)\n'
                            'def helper_ok(): return 1\ndef helper_new(): return 2\ndef test_observer(): assert True\n')
    case['proof']['observer_support'] = [module + '::helper_bad', module + '::helper_ok']
    error = planner_refusal(case, tmp_path)
    assert error.rule == 'proof.support.call_allowlist'
    repair = error.violations[0].repair
    assert isinstance(repair, Edit)
    assert repair.pointer.value == '/checks/3/proof/observer_support/0'
    assert module + '::helper_ok' not in repair.allowed.value
    assert all(isinstance(item, str) for item in repair.allowed.value)


def test_planner_subject_replacements_do_not_remove_obligation_subjects(tmp_path):
    from tl3_harness.rules import Edit
    case = partitions()['proof.subject.changed']['bad']
    case['after']['changed.py'] = 'def changed(): return 2\n'
    case['before']['changed.py'] = 'def changed(): return 1\n'
    repair = planner_refusal(case, tmp_path).violations[0].repair
    assert isinstance(repair, Edit)
    assert repair.pointer.value == '/checks/3/proof/subjects/0'
    assert all(isinstance(value, str) and value for value in repair.allowed.value)


def test_planner_node_candidates_are_collectable_and_never_empty(tmp_path):
    from tl3_harness.rules import Edit
    case = partitions()['proof.source.definition']['bad']
    case['after']['tests/test_observer.py'] += ('def helper(): pass\nclass Helper:\n    def test_hidden(self): pass\n'
                                               'class TestReal:\n    def helper(self): pass\n    def test_method(self): pass\n')
    repair = planner_refusal(case, tmp_path).violations[0].repair
    assert isinstance(repair, Edit)
    assert repair.pointer.value == '/checks/3/tests/0'
    assert all(isinstance(value, str) and value for value in repair.allowed.value)
    assert all('helper' not in value and 'Helper' not in value for value in repair.allowed.value)
    assert 'tests/test_observer.py::TestReal::test_method' in repair.allowed.value


def test_planner_probe_budget_and_changed_modules_first(tmp_path, monkeypatch):
    case = partitions()['proof.source.definition']['bad']
    for index in range(40):
        case['before'][f'tests/test_a{index:02}.py'] = 'def test_old(): pass\n'
        case['after'][f'tests/test_a{index:02}.py'] = 'def test_old(): pass\n'
    case['after']['tests/test_zchanged.py'] = 'def test_new(): assert True\n'
    probes = []
    error = planner_refusal(case, tmp_path, monkeypatch=monkeypatch, probes=probes)
    assert error.rule == 'proof.source.definition'
    assert len(probes) - 1 <= 25
    selections = error.violations[0].repair.allowed.value
    assert len(selections) <= 10
    assert 'tests/test_zchanged.py::test_new' in selections


@pytest.mark.parametrize('row_id', ['proof.source.definition', 'proof.observer.module_missing',
                                   'proof.observer.unresolved_symbol', 'proof.support.decorator'])
def test_maker_repairs_use_editable_observer_source(tmp_path, row_id):
    from tl3_harness.rules import SourceEdit
    case = partitions()[row_id]['bad']
    for side in ('before', 'after'):
        for name, text in case[side].items():
            path = tmp_path / side / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(text)
    check = model.Check('source repair', model.Kind.DEMONSTRATING, tuple(case['tests']), proof=model.Proof.parse(case['proof']))
    with pytest.raises(model.ProofRefusal) as caught:
        proof.assemble_proof(tmp_path / 'before', tmp_path / 'after', tmp_path / 'staged', check, case['config'], footprint=('tests/test_observer.py',))
    assert caught.value.rule == row_id
    repair = caught.value.violations[0].repair
    assert isinstance(repair, SourceEdit)
    assert repair.path == 'tests/test_observer.py'
    expected = {'proof.source.definition': 'define', 'proof.observer.module_missing': 'create',
                'proof.observer.unresolved_symbol': 'define', 'proof.support.decorator': 'fixture body',
                'proof.loader.stdlib_shadow': 'shadowing file'}[row_id]
    assert expected in repair.instruction.lower()


@pytest.mark.parametrize('protection', ['protected', 'generated'])
def test_source_repairs_respect_protected_and_generated_patterns(tmp_path, protection):
    from tl3_harness.rules import CantMeet
    case = partitions()['proof.support.call_allowlist']['bad']
    for side in ('before', 'after'):
        for name, text in case[side].items():
            path = tmp_path / side / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(text)
    check = model.Check('source repair', model.Kind.DEMONSTRATING, tuple(case['tests']), proof=model.Proof.parse(case['proof']))
    with pytest.raises(model.ProofRefusal) as caught:
        proof.assemble_proof(tmp_path / 'before', tmp_path / 'after', tmp_path / 'staged', check, case['config'],
                             footprint=('tests/test_observer.py',), **{protection: ('tests/*.py',)})
    assert caught.value.rule == 'proof.support.call_allowlist'
    assert isinstance(caught.value.violations[0].repair, CantMeet)


def test_planner_names_keep_the_editable_field_when_maker_would_edit_source(tmp_path):
    from tl3_harness.rules import evaluate
    row = ROWS['proof.support.call_allowlist']
    violation, = evaluate((row,), {'module': 'tests/test_observer.py', 'footprint': ('tests/test_observer.py',),
        'field': '/checks/3/proof/observer_support/0', 'role': 'planner'}, role='planner')
    assert any(name.kind == 'field' for name in row.names(violation.facts))


@pytest.mark.parametrize('owned,protected,generated,kind', [
    (False, (), (), 'CantMeet'), (True, (), (), 'SourceEdit'),
    (True, ('path*.py',), (), 'CantMeet'), (True, (), ('path*.py',), 'CantMeet'),
])
def test_stdlib_shadow_repairs_only_the_editable_shadow(tmp_path, owned, protected, generated, kind):
    from tl3_harness.rules import CantMeet, SourceEdit
    case = partitions()['proof.loader.stdlib_shadow']['bad']
    for side in ('before', 'after'):
        for name, source in case[side].items():
            path = tmp_path / side / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(source)
    check = model.Check('shadow', model.Kind.DEMONSTRATING, tuple(case['tests']), proof=model.Proof.parse(case['proof']))
    with pytest.raises(model.ProofRefusal) as caught:
        proof.assemble_proof(tmp_path / 'before', tmp_path / 'after', tmp_path / 'staged', check, case['config'],
                             footprint=('tests/test_observer.py', 'pathlib.py') if owned else ('tests/test_observer.py',),
                             protected=protected, generated=generated)
    assert caught.value.rule == 'proof.loader.stdlib_shadow'
    repair = caught.value.violations[0].repair
    assert isinstance(repair, {'CantMeet': CantMeet, 'SourceEdit': SourceEdit}[kind])
    if kind == 'SourceEdit':
        assert repair.path == 'pathlib.py'
    else:
        assert 'requires a plan change' in repair.reason



def test_source_repairs_respect_the_shared_generated_header_rule(tmp_path):
    from tl3_harness.rules import CantMeet
    case = partitions()['proof.support.call_allowlist']['bad']
    for side in ('before', 'after'):
        for name, source in case[side].items():
            path = tmp_path / side / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(('# @generated by fixture\n' if name == 'tests/test_observer.py' else '') + source)
    check = model.Check('generated', model.Kind.DEMONSTRATING, tuple(case['tests']), proof=model.Proof.parse(case['proof']))
    with pytest.raises(model.ProofRefusal) as caught:
        proof.assemble_proof(tmp_path / 'before', tmp_path / 'after', tmp_path / 'staged', check, case['config'], footprint=('tests/test_observer.py',))
    assert caught.value.rule == 'proof.support.call_allowlist'
    assert isinstance(caught.value.violations[0].repair, CantMeet)
