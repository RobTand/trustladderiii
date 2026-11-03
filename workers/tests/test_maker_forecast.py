# ruff: noqa: F811

from pathlib import Path
import pytest
from worker_process import start_worker
from message_invariants import cache_tokenizer as cache_tokenizer
from message_invariants import maker_request as maker_request
from message_invariants import requested_fit as requested_fit
from message_invariants import tool_capable_process_model as tool_capable_process_model
from message_invariants import turn as turn


@pytest.mark.parametrize('fit_tokens,linked,expected', [(100000, False, True), (1, False, False), (100000, True, False)])
def test_requested_file_fit_is_computed_by_worker_code(tmp_path, maker_request, cache_tokenizer, fit_tokens, linked, expected):
    maker_request['inputs']['forecast'] = {
        'fit_tokens': fit_tokens, 'context_tokens': 150000, 'overhead_tokens': {'reserve': 32000},
    }
    root = Path(maker_request['worktree'])
    if linked:
        (root / 'helper.py').symlink_to(root / 'greeting.py')
    result = start_worker(tmp_path, maker_request, [turn({'kind': 'needs_files', 'files': [{'path': 'helper.py', 'why': 'A shared helper is needed.'}]})],
                          env_overrides={'TIKTOKEN_CACHE_DIR': cache_tokenizer})
    assert result.exit_code == 0, result.stderr
    assert result.events[-1]['reply']['fits'] is expected


@pytest.mark.parametrize('content_source', ['brief', 'file'])
@pytest.mark.parametrize('cache', ['available', 'disabled', 'empty', 'corrupt'])
def test_requested_file_fit_counts_tokens_with_byte_fallback(tmp_path, maker_request, cache_tokenizer, content_source, cache):
    maker_request['inputs']['forecast'] = {
        'fit_tokens': 100000, 'context_tokens': 150000, 'overhead_tokens': {'reserve': 32000},
    }
    if content_source == 'brief':
        maker_request['inputs']['brief'] = 'token ' * 18000
    else:
        root = Path(maker_request['worktree'])
        for name in ('greeting.py', 'helper.py'):
            (root / name).write_text('token ' * 6000)
    directory = cache_tokenizer
    if cache == 'disabled':
        directory = ''
    elif cache in ('empty', 'corrupt'):
        directory = tmp_path / 'unavailable-tokenizer'
        directory.mkdir()
        if cache == 'corrupt':
            (directory / 'fb374d419588a4632f3f557e76b4b70aebbca790').write_bytes(b'bad cache')
        directory = str(directory)
    result = start_worker(tmp_path, maker_request, [turn({
        'kind': 'needs_files', 'files': [{'path': 'helper.py', 'why': 'Needed.'}],
    })], env_overrides={'TIKTOKEN_CACHE_DIR': directory})
    assert result.exit_code == 0, result.stderr
    assert result.events[-1]['reply']['fits'] is (cache == 'available')
    assert result.stderr == ''


def test_model_cannot_claim_that_requested_files_fit(tmp_path, maker_request):
    maker_request['inputs']['forecast'] = {
        'fit_tokens': 100000, 'context_tokens': 150000, 'overhead_tokens': {'reserve': 32000},
    }
    result = start_worker(tmp_path, maker_request, [turn({'kind': 'needs_files', 'files': [{'path': 'helper.py', 'why': 'Needed.'}], 'fits': True})])
    assert result.exit_code == 1
    assert all(event['event'] != 'reply' for event in result.events)


@pytest.mark.parametrize('extra_tokens,expected', [(0, True), (4000, True), (18000, False)])
def test_pq_2629_plan_and_requested_file_fit(maker_request, cache_tokenizer, monkeypatch, extra_tokens, expected):
    from tl3_workers.forecast import Forecaster

    monkeypatch.setenv('TIKTOKEN_CACHE_DIR', cache_tokenizer)
    root = Path(maker_request['worktree'])
    monkeypatch.chdir(root)
    inputs = maker_request['inputs']
    config = {'fit_tokens': 100000, 'context_tokens': 150000,
              'overhead_tokens': {'prompt': 5000, 'making': 20000}}
    inputs['forecast'] = config
    inputs['brief'] = 'x\n' * 1643
    # Model PQ #2629's eight files: two ranged files and six small files.
    costs = dict(zip([f'plan_{i}.py' for i in range(8)], [16384, 16384, 4000, 4000, 4000, 4000, 4000, 2232]))
    for index, (path, tokens) in enumerate(costs.items()):
        (root / path).write_text('large ' * 14000 if index < 2 else 'x\n' * (tokens // 2))
    inputs['plan']['files'] = list(costs)
    # The remaining input brought the old maker JSON count to about 25k.
    inputs['goals'][0]['description'] = ' token' * 21000
    inputs.update(round=1, feedback=[{'kind': 'failed_check', 'report': 'Fix the check.', 'goal_ids': ['greeting']}])
    forecast = Forecaster(config).measure(inputs['brief'], inputs['plan']['files'])
    assert forecast['brief_tokens'] == 3286
    assert forecast['file_tokens'] == costs
    assert forecast['tokens'] == 83286
    assert forecast['fits'] is True
    (root / 'requested.py').write_text('x\n' * (extra_tokens // 2))
    paths = [*inputs['plan']['files'], 'requested.py'] if extra_tokens else inputs['plan']['files']
    assert requested_fit(maker_request, paths) is expected


@pytest.mark.parametrize('overhead', [0, 25000, 45000])
@pytest.mark.parametrize('fix_round', [False, True])
def test_planner_fit_implies_maker_fit(maker_request, cache_tokenizer, monkeypatch, overhead, fix_round):
    from tl3_workers.forecast import Forecaster

    monkeypatch.setenv('TIKTOKEN_CACHE_DIR', cache_tokenizer)
    monkeypatch.chdir(maker_request['worktree'])
    inputs = maker_request['inputs']
    config = {'fit_tokens': 100000, 'context_tokens': 150000,
              'overhead_tokens': {'reserve': overhead}}
    total = Forecaster(config).measure(inputs['brief'], inputs['plan']['files'])['tokens']
    config['fit_tokens'] = total + 1
    inputs['forecast'] = config
    if fix_round:
        inputs.update(round=1, feedback=[{'kind': 'failed_check', 'report': 'Fix it. ' * 2000, 'goal_ids': ['greeting']}])
    assert Forecaster(config).measure(inputs['brief'], inputs['plan']['files'])['fits'] is True
    assert requested_fit(maker_request, inputs['plan']['files']) is True
