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
