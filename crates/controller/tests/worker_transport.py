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
    if request['role'] == 'maker':
        # Steering also checks publication of an actual commit. Keep that boundary real.
        result = subprocess.run(
            [sys.executable, str(Path(__file__).with_name('worker_model.py')), mode, str(marker)],
            input=json.dumps(request), text=True,
        )
        return result.returncode
    reply = scripted_reply(request, mode, marker)
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
