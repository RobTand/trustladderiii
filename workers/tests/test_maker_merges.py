# ruff: noqa: F811

import json
from pathlib import Path
import subprocess
from langchain_core.messages import AIMessage
import pytest
from tl3_workers import runtime
from message_invariants import DONE as DONE
from message_invariants import ToolModel as ToolModel
from message_invariants import git as git
from message_invariants import maker_request as maker_request
from message_invariants import pending_conflict as pending_conflict
from message_invariants import tool_capable_process_model as tool_capable_process_model
from message_invariants import turn as turn


def test_conflict_resolution_commits_merge_even_when_keeping_leaf_tree(maker_request):
    from tl3_workers.roles import maker

    root = Path(maker_request['worktree'])
    base = git(root, 'rev-parse', 'HEAD')
    (root / 'greeting.py').write_text("def greet(): return 'hello'\n")
    git(root, 'add', '.')
    git(root, 'commit', '-m', 'Leaf')
    leaf = git(root, 'rev-parse', 'HEAD')
    branch = git(root, 'symbolic-ref', '--short', 'HEAD')
    git(root, 'checkout', '-b', 'upstream', base)
    (root / 'greeting.py').write_text("def greet(): return 'bonjour'\n")
    git(root, 'add', '.')
    git(root, 'commit', '-m', 'Main')
    target = git(root, 'rev-parse', 'HEAD')
    git(root, 'checkout', branch)
    with pytest.raises(subprocess.CalledProcessError):
        git(root, 'merge', '--no-commit', target)
    (root / 'greeting.py').write_text("def greet(): return 'hello'\n")
    head = maker._commit(maker_request, leaf)
    assert head and head != leaf, 'a resolution retaining the leaf tree must still finish the merge'
    assert git(root, 'rev-parse', 'HEAD^1') == leaf
    assert git(root, 'rev-parse', 'HEAD^2') == target
    assert git(root, 'status', '--porcelain') == ''


@pytest.mark.parametrize('failure', ['markers', 'aborted', 'committed-markers'])
def test_pending_merge_done_requires_resolution_and_original_target(maker_request, monkeypatch, failure):
    root, leaf, target = pending_conflict(maker_request)
    class InvalidResolution(ToolModel):
        def _generate(self, *args, **kwargs):
            if failure == 'aborted':
                git(root, 'merge', '--abort')
                (root / 'greeting.py').write_text("def greet(): return 'edited on old base'\n")
            elif failure == 'committed-markers':
                git(root, 'add', '.')
                git(root, 'commit', '-m', 'Unresolved markers')
            return super()._generate(*args, **kwargs)
    monkeypatch.setattr(runtime, 'create_model', lambda _: InvalidResolution(responses=[AIMessage(**turn(DONE))]))
    events = []
    runtime.run(maker_request, events.append)
    reply = events[-1]['reply']
    assert reply['kind'] == 'cant_meet', 'an unresolved or aborted merge must never report done'
    assert 'merge' in reply['report'].lower()
    if failure == 'markers':
        assert git(root, 'rev-parse', 'HEAD') == leaf, 'worker must not commit unresolved conflict markers'
        assert git(root, 'rev-parse', 'MERGE_HEAD') == target


def test_merge_target_survives_crash_after_an_aborted_resolution(maker_request, monkeypatch, tmp_path):
    root, leaf, target = pending_conflict(maker_request)
    maker_request['session_file'] = str(tmp_path / 'session.json')
    class AbortAndCrash(ToolModel):
        def _generate(self, *args, **kwargs):
            git(root, 'merge', '--abort')
            (root / 'greeting.py').write_text("def greet(): return 'old base edit'\n")
            raise RuntimeError('crash after abort')
    monkeypatch.setattr(runtime, 'create_model', lambda _: AbortAndCrash(responses=[]))
    with pytest.raises(RuntimeError, match='crash after abort'):
        runtime.run(maker_request, lambda _: None)
    monkeypatch.setattr(runtime, 'create_model', lambda _: ToolModel(responses=[AIMessage(**turn(DONE))]))
    events = []
    runtime.run(maker_request, events.append)
    assert events[-1]['reply']['kind'] == 'cant_meet', 'retry must retain the original pending merge target'
    assert git(root, 'rev-parse', 'HEAD') == leaf


@pytest.mark.parametrize('documentation', [
    'Title\n=========\n',
    'Marker examples\n<<<<<<< example\n>>>>>>> example\n',
])
def test_resolved_merge_ignores_unconflicted_documentation(maker_request, monkeypatch, documentation):
    root = Path(maker_request['worktree'])
    (root / 'README.rst').write_text(documentation)
    git(root, 'add', 'README.rst')
    git(root, 'commit', '-m', 'Document the project')
    root, leaf, target = pending_conflict(maker_request)
    class Resolved(ToolModel):
        def _generate(self, *args, **kwargs):
            (root / 'greeting.py').write_text("def greet(): return 'resolved'\n")
            git(root, 'add', 'greeting.py')
            return super()._generate(*args, **kwargs)
    monkeypatch.setattr(runtime, 'create_model', lambda _: Resolved(responses=[AIMessage(**turn(DONE))]))
    events = []
    runtime.run(maker_request, events.append)
    assert events[-1]['reply']['kind'] == 'done', 'unconflicted documentation must not block a resolved merge'
    assert git(root, 'rev-parse', 'HEAD^1') == leaf
    assert git(root, 'rev-parse', 'HEAD^2') == target
    assert (root / 'README.rst').read_text() == documentation


@pytest.mark.parametrize('legacy_checkpoint', [False, True])
def test_original_conflict_files_survive_staging_and_restart(maker_request, monkeypatch, tmp_path, legacy_checkpoint):
    root, leaf, target = pending_conflict(maker_request)
    session = tmp_path / 'session.json'
    maker_request['session_file'] = str(session)
    class StageAndCrash(ToolModel):
        def _generate(self, *args, **kwargs):
            (root / 'greeting.py').write_text('<<<<<<< leftover\nnot resolved\n')
            git(root, 'add', 'greeting.py')
            raise RuntimeError('crash after staging')
    monkeypatch.setattr(runtime, 'create_model', lambda _: StageAndCrash(responses=[]))
    with pytest.raises(RuntimeError, match='crash after staging'):
        runtime.run(maker_request, lambda _: None)
    checkpoint = session.with_suffix('.head')
    saved = json.loads(checkpoint.read_text())
    assert saved.get('merge_files') == ['greeting.py'], 'the checkpoint must retain the originally unmerged files'
    if legacy_checkpoint:
        del saved['merge_files']
        checkpoint.write_text(json.dumps(saved))
    monkeypatch.setattr(runtime, 'create_model', lambda _: ToolModel(responses=[AIMessage(**turn(DONE))]))
    events = []
    runtime.run(maker_request, events.append)
    assert events[-1]['reply']['kind'] == 'cant_meet', 'staging must not hide leftover markers on restart'
    assert git(root, 'rev-parse', 'HEAD') == leaf
