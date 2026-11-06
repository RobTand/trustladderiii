"""#899 review regression coverage at the assembly and result boundaries."""
import json
from pathlib import Path

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
    check = model.Check('row witness', model.Kind.DEMONSTRATING, tuple(case['tests']), proof=model.Proof.parse(case['proof']))
    return proof.assemble_proof(root / 'before', root / 'after', destination, check, case['config'])


@pytest.mark.parametrize('row_id', [id for id in ROWS if id not in ('forcing.raise_class', 'proof.staging.single_check')])
def test_row_has_real_assembly_input_and_source_repair(tmp_path, row_id):
    case = partitions()[row_id]
    with pytest.raises(model.ProofRefusal) as caught:
        assemble(case['bad'], tmp_path / 'bad')
    assert any(v.rule.id == row_id for v in caught.value.violations)
    assert caught.value.rule == row_id
    for v in caught.value.violations:
        message = v.render(role='maker')
        assert row_id in message and 'Allowed:' in message
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
