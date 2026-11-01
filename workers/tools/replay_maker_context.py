"""Paired 50-tool-turn maker cost replay on a pinned PrismaBuild checkout.

The model reads real repository files, writes an isolated marker and reads it
back before done. This exercises retention and code-owned commits; it does not
implement #1968 or claim its checks passed. Run only inside PrismaBuild.
"""
import json
from pathlib import Path
import subprocess
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'tests'))
import worker_process


INSPECT = '''
from dataclasses import replace
sys.path.insert(0, TOOLS_PATH)
from replay_usage import ReplayUsage
from tl3_workers.roles import maker, ROLES
from langchain_core.utils.function_calling import convert_to_openai_tool
if BASELINE:
    maker.ROLE = replace(maker.ROLE, exploration_budget=None)
    ROLES['maker'] = maker.ROLE
accounting = ReplayUsage(cached=not BASELINE)
metrics_path = Path(sys.argv[1]).with_suffix('.metrics.json')
metrics, offered = [], []
class InspectModel(FakeMessagesListChatModel):
    def bind_tools(self, tools, **kwargs):
        offered[:] = [convert_to_openai_tool(t) for t in tools]
        return self

    def _generate(self, messages, **kwargs):
        record, usage = accounting.measure(messages, offered)
        metrics.append(record)
        metrics_path.write_text(json.dumps(metrics))
        response = super()._generate(messages, **kwargs)
        response.generations[0].message.usage_metadata = usage
        return response
'''


def replay(source, baseline):
    with tempfile.TemporaryDirectory(prefix='tl3-maker-cost-') as directory:
        root = Path(directory)
        checkout = root / 'checkout'
        subprocess.run(['git', 'clone', '--quiet', '--no-hardlinks', str(source), str(checkout)], check=True)
        before = subprocess.check_output(['git', '-C', str(checkout), 'rev-parse', 'HEAD'], text=True).strip()
        fixture = json.loads((Path(__file__).resolve().parents[1] / 'tests/fixtures/prismabuild-1968.json').read_text())
        marker = 'tl3-context-replay.txt'
        text = 'Offline context replay completed.\n'
        reply = {'kind': 'done', 'self_check': [{'goal_id': 'marker', 'status': 'met',
                 'evidence': 'The marker was written and read back. No behavioral checks were run.'}]}
        calls = [('read', {'path': path, 'offset': offset, 'limit': 200})
                 for _ in range(8) for offset in (0, 200, 400)
                 for path in ('tools/fleet/docker', 'tests/test_docker_shim_global_options.py')]
        calls += [('write', {'path': marker, 'content': text}), ('read', {'path': marker})]
        script = [{'content': '', 'tool_calls': [{'name': name, 'args': args, 'id': str(i),
                   'type': 'tool_call'}]} for i, (name, args) in enumerate(calls)]
        script.append({'content': json.dumps(reply)})
        request = {
            'protocol': 1, 'operation': {'node': 0, 'generation': 0, 'kind': 'make'},
            'role': 'maker', 'worktree': str(checkout), 'key_file': str(root / 'never-read-key'),
            'model': {'base_url': 'http://model.invalid/v1', 'name': 'scripted-offline',
                      'effort': 'high', 'budget_tokens': 32000},
            'inputs': {'brief': 'Offline cost fixture for the following issue. Read the named repository '
                      'files, then create the isolated marker. Do not claim the issue is implemented.\n' + fixture['body'],
                       'goals': [{'id': 'marker', 'description': 'Write and read back the offline marker.',
                                  'verification': 'review'}],
                       'plan': {'files': [marker], 'checks': [{'name': 'fixture', 'kind': 'demonstrating',
                                'tests': ['tests/test_docker_shim_global_options.py'], 'goal_ids': ['marker']}]},
                       'attempt': 1, 'round': 0, 'context_size': 2_000_000,
                       'tests': {'image': 'sha256:' + 'a' * 64,
                                 'tools_path': '/mnt/shared/prismabuild-fleet/repo/tools'}},
        }
        original = worker_process.LAUNCH
        worker_process.LAUNCH = original.replace('model = FakeMessagesListChatModel(',
            INSPECT.replace('BASELINE', repr(baseline)).replace('TOOLS_PATH', repr(str(Path(__file__).resolve().parent))) + '\nmodel = InspectModel(')
        try:
            result = worker_process.start_worker(root, request, script)
        finally:
            worker_process.LAUNCH = original
        assert result.exit_code == 0, result.stderr
        assert not Path(request['key_file']).exists()
        head = subprocess.check_output(['git', '-C', str(checkout), 'rev-parse', 'HEAD'], text=True).strip()
        assert result.events[-1]['reply'] == {**reply, 'head': head}
        assert head != before
        assert subprocess.check_output(['git', '-C', str(checkout), 'show', f'HEAD:{marker}'], text=True) == text
        metrics = json.loads((root / 'scripted-model.metrics.json').read_text())
        assert len(metrics) == 51
        report = {'role': 'maker', 'issue_fixture': 1968, 'baseline': baseline, 'base': before,
                  'model_turns': len(metrics), 'tool_turns': len(calls),
                  'input_tokens': sum(m['input_tokens'] for m in metrics),
                  'weighted_tokens': sum(m['weighted_tokens'] for m in metrics),
                  'cache_read': sum(m['cache_read'] for m in metrics),
                  'cache_creation': sum(m['cache_creation'] for m in metrics),
                  'prefix_breaks': sum(not m['prefix_preserved'] for m in metrics[1:]),
                  'largest_context': max(m['input_tokens'] for m in metrics),
                  'max_history_bytes': max(m['history_bytes'] for m in metrics), 'exit_code': result.exit_code}
        print(json.dumps(report), flush=True)
        return report


if __name__ == '__main__':
    source = Path(sys.argv[1]).resolve()
    before = replay(source, True)
    after = replay(source, False)
    assert after['prefix_breaks'] == 0
    assert after['weighted_tokens'] < before['weighted_tokens']
    assert after['weighted_tokens'] <= 6_000_000
