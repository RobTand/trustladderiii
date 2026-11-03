# ruff: noqa: F811
"""Planning controls from the saved #645 corpus, including correct refusals."""
from copy import deepcopy
import json
from pathlib import Path

from worker_process import rehearsal  # noqa: F401

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


def test_maker_and_rehearsal_use_the_same_source_partition_as_checking(tmp_path):
    import json
    import shutil
    from tl3_harness.model import Check, Forcing, Kind
    from tl3_harness.proof import assemble_proof
    from tl3_harness.trees import export, git, configuration
    from tl3_workers.pb_tests import _stage
    from tl3_workers.tools import Worktree

    repo = tmp_path / 'repo'
    repo.mkdir()
    (repo / 'pyproject.toml').write_text(
        '[tool.tl3.harness]\ncommand=["python", "-m", "pytest"]\ntest-files=["test_*.py"]\n')
    (repo / 'test_queue.py').write_text('def helper(): return "old"\n')
    git(repo, 'init', '-q')
    git(repo, 'add', '.')
    git(repo, '-c', 'user.name=Fixture', '-c', 'user.email=f@example.invalid', 'commit', '-qm', 'base')
    base_sha = git(repo, 'rev-parse', 'HEAD').decode().strip()
    (repo / 'test_queue.py').write_text('def helper(): return "new"\ndef test_value(): assert helper() == "new"\n')
    declaration = {'name': 'proof', 'kind': 'demonstrating', 'tests': ['test_queue.py::test_value']}
    stage = tmp_path / 'maker'
    stage.mkdir()
    with Worktree(repo) as tree:
        tree.test_checkout = repo
        _stage(tree, stage, base_sha, checks=[declaration], selected=tuple(declaration['tests']))
    staged = (stage / 'checkout/test_queue.py').read_text()
    assert 'return "old"' in staged and 'assert helper() == "new"' in staged
    base = tmp_path / 'base'
    export(repo, base_sha, base)
    head = tmp_path / 'head'
    shutil.copytree(repo, head, ignore=shutil.ignore_patterns('.git'))
    receipt = assemble_proof(base, head, tmp_path / 'checking',
                             Check('proof', Kind.DEMONSTRATING, tuple(declaration['tests'])), configuration(base))
    assert json.loads((stage / 'proof.json').read_text()) == receipt
    symptom_stage = tmp_path / 'maker-symptom'
    symptom_stage.mkdir()
    symptom = {'node': 'test_queue.py::test_value', 'exception': 'builtins.TimeoutError',
               'phase': 'call', 'repeat': 1, 'parallel': 0}
    with Worktree(repo) as tree:
        tree.test_checkout = repo
        _stage(tree, symptom_stage, base_sha, checks=[{**declaration, 'declared_symptom': symptom}],
               selected=tuple(declaration['tests']))
    symptom_receipt = json.loads((symptom_stage / 'proof.json').read_text())
    assert symptom_receipt['mode'] == 'pytest-outcome'
    assert (symptom_stage / 'checkout/test_queue.py').read_bytes() == (base / 'test_queue.py').read_bytes()
    forcing = Forcing.parse({'symptom': 'old subject', 'patches': [{'target': 'queue.clock', 'returns': 0}]})
    rehearsal = tmp_path / 'rehearsal'
    rehearsal.mkdir()
    with Worktree(repo) as tree:
        tree.test_checkout = repo
        _stage(tree, rehearsal, base_sha, selected=('test_queue.py::test_original',), forcing=forcing)
    proof = json.loads((rehearsal / 'proof.json').read_text())
    assert proof['mode'] == 'pytest-outcome'
    assert proof['base_tree_sha256'] == proof['source_base_tree_sha256'] == proof['head_tree_sha256']
    assert (rehearsal / 'checkout/test_queue.py').read_bytes() == (base / 'test_queue.py').read_bytes()


def test_review662_mixed_checks_keep_their_own_source_partitions(rehearsal, monkeypatch, tmp_path):
    from tl3_harness.model import Check, Forcing, Kind, Proof
    from tl3_harness.proof import assemble_proof
    from tl3_harness.trees import export, configuration
    from tl3_workers import pb_tests
    ledger, root, inputs, _ = rehearsal
    path = 'tests/test_adoption.py'
    node = path + '::test_new_observer'
    (root / path).write_text((root / path).read_text() + '\ndef test_new_observer(): assert False, "new observer"\n')
    checks = [{'name': 'observer', 'kind': 'demonstrating', 'tests': [node],
               'proof': {'subjects': ['publisher.py::Publisher'], 'observer_support': [], 'red_message': 'new observer'}},
              {'name': 'forced', 'kind': 'demonstrating', 'tests': [path + '::test_adopt'],
               'forcing': {'symptom': 'old helper', 'environment': {'CASE': 'race'}}}]
    # The first subject needs to differ without being copied into the base proof.
    (root / 'publisher.py').write_text((root / 'publisher.py').read_text() + '\n# changed source\n')
    checks[0]['proof']['subjects'] = ['publisher.py']
    ledger.worktree.test_checks = checks
    staged = []
    stage = pb_tests._stage
    def capture(*args, **kwargs):
        result = stage(*args, **kwargs)
        destination = args[1]
        staged.append(((destination / 'checkout' / path).read_bytes(), json.loads((destination / 'proof.json').read_text())))
        return result
    monkeypatch.setattr(pb_tests, '_stage', capture)
    # No rehearsal bookkeeping: this is the maker's run_tests path.
    ledger.worktree.pb_submitted = lambda *args: None
    pb_tests.make_test_tool(ledger.worktree)([path], base=True)
    base = tmp_path / 'checking-base'
    export(root, inputs['base_sha'], base)
    expected = []
    for index, item in enumerate(checks):
        destination = tmp_path / f'checking-{index}'
        receipt = assemble_proof(base, root, destination,
            Check(item['name'], Kind(item['kind']), tuple(item['tests']),
                  Forcing.parse(item['forcing']) if 'forcing' in item else None,
                  Proof.parse(item['proof']) if 'proof' in item else None), configuration(base))
        expected.append(((destination / path).read_bytes(), receipt))
    assert staged == expected, 'maker and Checking must assemble the same per-check trees and receipts'


def test_review662_request229_replays_saved_contract_and_untouched_setup_error(rehearsal):
    from worker_process import git
    from tl3_workers.forcing_rehearsal import ForcingRehearsals
    ledger, root, inputs, calls = rehearsal
    path = 'tests/test_paced_exports_share_a_tier.py'
    node = path + '::test_a_finished_export_files_its_rate_and_prices_the_next_seal'
    (root / 'produced_spool.py').write_text('class SpoolError(Exception): pass\ndef _local_disk(root):\n    raise SpoolError("spool root must be on a local disk filesystem")\n')
    (root / path).write_text('import pytest\nfrom produced_spool import _local_disk\n@pytest.fixture\ndef world_roomy(tmp_path):\n    return _local_disk(tmp_path)\ndef test_a_finished_export_files_its_rate_and_prices_the_next_seal(world_roomy):\n    assert world_roomy\n')
    git(root, 'add', '.')
    git(root, 'commit', '-qm', 'request 229 untouched base')
    inputs['base_sha'] = git(root, 'rev-parse', 'HEAD')
    fixture = Path(__file__).parents[2] / 'harness/tests/fixtures/subject_observer'
    saved = json.loads((fixture / 'request229-contract.json').read_text())
    inputs['ingest']['evidence_contract'] = saved
    inputs['tool_capabilities'] = [{**deepcopy(saved[0]), 'runner_features': ['base_head', 'repeat', 'parallel']}]
    reply = {'kind': 'leaf', 'checks': [], 'evidence_contract': []}
    preflight(inputs, reply, planning=True)
    assert reply['kind'] == 'leaf' and reply['evidence_errors'] == []
    ledger = ForcingRehearsals(ledger.worktree, inputs)
    result = ledger.run([node], {'symptom': 'paced export stall', 'environment': {'CASE': 'race'}})
    assert result['status'] == 'refused' and 'unforced base' in result['message'], result
    assert len(calls) == 1 and result['unforced']['totals']['error'] == 1, result
    assert 'SpoolError' in result['unforced']['tests'][0]['message'], result
    corpus = json.loads((fixture / 'corpus.json').read_text())
    assert next(item for item in corpus if item['request'] == 229)['source_obligations'] == saved
