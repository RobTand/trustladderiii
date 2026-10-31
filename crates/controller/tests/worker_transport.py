"""Protocol-only worker for controller steering tests.

State routing, persistence, retries and process supervision remain real. Agent
execution and token measurement have separate real-worker contract scenarios.
"""
import json
import os
from pathlib import Path
import subprocess
import sys

from worker_replies import scripted_reply


def main():
    mode, marker = sys.argv[1], Path(sys.argv[2])
    request = json.load(sys.stdin)
    reply = scripted_reply(request, mode, marker)
    if request['role'] == 'maker':
        tree = Path(request['worktree'])
        (tree / 'greeting.py').write_text("def greet(): return 'hello'\n")
        env = {key: value for key, value in os.environ.items() if not key.startswith('GIT_')}
        env.update(GIT_CONFIG_NOSYSTEM='1', GIT_CONFIG_GLOBAL=os.devnull,
                   GIT_AUTHOR_NAME='Fixture', GIT_AUTHOR_EMAIL='fixture@example.invalid',
                   GIT_COMMITTER_NAME='Fixture', GIT_COMMITTER_EMAIL='fixture@example.invalid')
        def git(*args):
            return subprocess.check_output(
                ['git', '-c', 'core.hooksPath=/dev/null', '-c', 'commit.gpgsign=false', *args],
                cwd=tree, env=env, text=True,
            ).strip()
        git('add', 'greeting.py')
        git('commit', '--quiet', '--allow-empty', '-m', 'Scripted steering change')
        reply = {'kind': 'done', 'head': git('rev-parse', 'HEAD'), 'self_check': [
            {'goal_id': goal['id'], 'status': 'met', 'evidence': 'Scripted greeting change.'}
            for goal in request['inputs']['goals']
        ]}
        if 'session_file' in request:
            session = Path(request['session_file'])
            session.parent.mkdir(parents=True, exist_ok=True)
            session.write_text('[]')
    config = request['inputs'].get('forecast')
    if config and request['role'] in {'ingest', 'planner'}:
        paths = reply.get('files', [])
        overhead = sum(config['overhead_tokens'].values())
        tokens = 1 + len(paths) + overhead
        reply['forecast'] = {
            'tokenizer': 'o200k_base', 'brief_tokens': 1,
            'file_tokens': dict.fromkeys(paths, 1), 'overhead_tokens': overhead,
            'tokens': tokens, 'fit_tokens': config['fit_tokens'],
            'context_tokens': config['context_tokens'], 'fits': tokens < config['fit_tokens'],
        }
    if request['role'] == 'planner':
        reply['violations'] = []
    print(json.dumps({'event': 'reply', 'role': request['role'], 'reply': reply}), flush=True)
    return 0


if __name__ == '__main__':
    try:
        raise SystemExit(main())
    except Exception:
        # Failure is part of the classifier retry fixture, just as at the real boundary.
        os._exit(1)
