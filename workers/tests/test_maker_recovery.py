# ruff: noqa: F811

import json
from pathlib import Path
import pytest
import worker_process
from worker_process import start_worker
from message_invariants import DONE as DONE
from message_invariants import git as git
from message_invariants import maker_request as maker_request
from message_invariants import tool_capable_process_model as tool_capable_process_model
from message_invariants import turn as turn
from message_invariants import write_turn as write_turn


@pytest.mark.parametrize('failure', ['killed', 'session-save'])
@pytest.mark.parametrize('failures', [1, 2])
@pytest.mark.parametrize('fix_round', [False, True])
def test_new_generation_recovers_commit_before_reply(tmp_path, maker_request, monkeypatch, failure, failures, fix_round):
    root = Path(maker_request['worktree'])
    base = git(root, 'rev-parse', 'HEAD')
    maker_request['session_file'] = str(tmp_path / 'session.json')
    maker_request['inputs']['context_size'] = 150000
    launcher = worker_process.LAUNCH
    if fix_round:
        accepted = start_worker(tmp_path, maker_request, [write_turn(), turn(DONE)])
        assert accepted.exit_code == 0, accepted.stderr
        maker_request['last_accepted_generation'] = maker_request['operation']['generation']
        maker_request['operation']['generation'] += 1
        maker_request['inputs']['round'] = 1
        maker_request['inputs']['feedback'] = [{'kind': 'failed_check', 'report': 'Repair the greeting.', 'goal_ids': ['greeting']}]
        maker_request['resume_session'] = True
    visit_base = git(root, 'rev-parse', 'HEAD')
    visit_generation = maker_request['operation']['generation']
    injection = """
from tl3_workers.roles import maker
commit = maker._commit
save = maker.save
def crash_commit(*args):
    commit(*args)
    import os, signal
    os.kill(os.getpid(), signal.SIGKILL)
def fail_save(path, value):
    if path.suffix == '.json':
        raise OSError('injected session save failure')
    save(path, value)
"""
    injection += ("maker._commit = crash_commit\n" if failure == 'killed'
                  else "maker.save = fail_save\n")
    monkeypatch.setattr(worker_process, 'LAUNCH', launcher.replace('def no_network', injection + '\ndef no_network'))
    for index in range(failures):
        turns = [write_turn(content="def greet(): return 'hello fixed'\n")] if index == 0 else []
        failed = start_worker(tmp_path, maker_request, [*turns, turn(DONE)])
        assert failed.exit_code == (-9 if failure == 'killed' else 1), failed.stderr
        assert all(event['event'] != 'reply' for event in failed.events)
        head = git(root, 'rev-parse', 'HEAD')
        assert head != visit_base
        assert git(root, 'rev-list', '--count', f'{visit_base}..HEAD') == '1'
        maker_request['operation']['generation'] += 1
    monkeypatch.setattr(worker_process, 'LAUNCH', launcher)
    recovered = start_worker(tmp_path, maker_request, [turn(DONE)])
    assert recovered.exit_code == 0, recovered.stderr
    assert recovered.events[-1]['reply'] == {**DONE, 'head': head}
    assert git(root, 'rev-list', '--count', f'{base}..HEAD') == str(2 if fix_round else 1)
    assert git(root, 'rev-parse', 'HEAD') == head
    checkpoint = Path(maker_request['session_file']).with_suffix('.head')
    assert json.loads(checkpoint.read_text()) == {'generation': visit_generation, 'head': visit_base}
