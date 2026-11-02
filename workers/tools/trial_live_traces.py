"""Check #430's live traces against a real MLflow server, in a scratch experiment.

Submit through PrismaBuild, pinned to dl380g10, and run with `python -I`. It
loads no credentials and writes only to experiment `tl3-430-scratch`, which the
shipper creates if it is absent. Server responses are untrusted: each is saved
in a fresh directory and parsed from there.

1. A run with two model turns: the trace is IN_PROGRESS with the run's tags
   after the first turn, then OK with both turns and summed tokens.
2. A worker killed after its first turn: the flusher the controller starts
   closes its trace as ERROR with outcome `crashed`. A second flush adds nothing.
"""
import argparse
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
from uuid import uuid4

import httpx

EXPERIMENT = 'tl3-430-scratch'
USAGE = {'input_tokens': 11, 'output_tokens': 7, 'total_tokens': 18}


def request(uri, log, run):
    return {
        'role': 'maker', 'model': {'name': 'synthetic-430', 'effort': 'high'},
        'operation': {'node': 430, 'generation': 0},
        'inputs': {'brief': 'Synthetic run for live traces.'},
        'mlflow': {'tracking_uri': uri, 'experiment': EXPERIMENT},
        'trace': {'repo': 'synthetic-430', 'issue': 430, 'request': run, 'attempt': 1, 'round': 0,
                  'log': str(log)},
    }


def turn(mlflow, index):
    with mlflow.start_span(name='model', span_type='CHAT_MODEL') as span:
        span.set_inputs({'messages': [{'role': 'user', 'content': f'turn {index}'}]})
        span.set_outputs({'content': f'reply {index}'})
        span.set_attribute('mlflow.chat.tokenUsage', USAGE)


class Reader:
    """Read-only views of one trace, each response kept in its own file."""

    def __init__(self, uri, directory):
        self.client = httpx.Client(base_url=uri, timeout=10, trust_env=False)
        self.directory, self.count = directory, 0

    def saved(self, response):
        self.count += 1
        path = self.directory / f'{self.count:03d}.json'
        path.write_bytes(response.content)
        return json.loads(path.read_text()) if response.status_code == 200 else None

    def info(self, trace_id):
        data = self.saved(self.client.get(f'/api/3.0/mlflow/traces/{trace_id}'))
        return data['trace']['trace_info'] if data else None

    def spans(self, trace_id):
        data = self.saved(self.client.get('/ajax-api/2.0/mlflow/get-trace-artifact',
                                          params={'request_id': trace_id}))
        return data.get('spans', []) if data else []

    def until(self, trace_id, accept):
        # A live check has no event from the remote store, so it retries a bounded number of times.
        for _ in range(60):
            info = self.info(trace_id)
            if info and accept(info):
                return info
            time.sleep(0.5)
        raise AssertionError(f'{trace_id} never reached the expected state: {info}')


def trace_id(log):
    header = json.loads(Path(log).read_text().splitlines()[0])
    return 'tr-' + header['trace_id']


def summary(info, spans):
    tags = info.get('tags', {})
    keep = ('role', 'model', 'repo', 'issue', 'request', 'node', 'generation', 'attempt', 'round',
            'outcome', 'total_tokens', 'failure_class', 'failure_message')
    return {'state': info.get('state'), 'tags': {key: tags.get(key) for key in keep},
            'spans': sorted(span.get('name', '') for span in spans)}


def live(uri, reader, root):
    import mlflow
    from tl3_workers.tracing import observe, record_reply

    run = 'live-' + uuid4().hex[:12]
    log = root / 'live' / 'trace.jsonl'
    log.parent.mkdir()
    with observe(request(uri, log, run)):
        turn(mlflow, 1)
        trace = trace_id(log)
        during = reader.until(trace, lambda info: any(
            span.get('name') == 'tl3_maker:synthetic-430' for span in reader.spans(trace)))
        assert during['state'] == 'IN_PROGRESS', during['state']
        assert during['tags'].get('request') == run and during['tags'].get('role') == 'maker'
        mid = summary(during, reader.spans(trace))
        turn(mlflow, 2)
        record_reply({'kind': 'done'})
    final = reader.until(trace, lambda info: info['state'] != 'IN_PROGRESS')
    spans = reader.spans(trace)
    assert final['state'] == 'OK', final['state']
    assert final['tags'].get('outcome') == 'succeeded' and final['tags'].get('total_tokens') == '36'
    assert sum(span.get('name') == 'tl3_maker:synthetic-430' for span in spans) == 2
    return {'trace': trace, 'mid_run': mid, 'final': summary(final, spans)}


CRASH = """
import os, signal, sys
import mlflow
sys.path.insert(0, sys.argv[2])
from trial_live_traces import request, turn
from tl3_workers.tracing import observe
with observe(request(sys.argv[1], sys.argv[3], sys.argv[4])):
    turn(mlflow, 1)
    os.kill(os.getpid(), signal.SIGKILL)
"""


def crashed(uri, reader, root):
    run = 'crash-' + uuid4().hex[:12]
    log = root / 'crash' / 'trace.jsonl'
    log.parent.mkdir()
    worker = subprocess.run([sys.executable, '-I', '-c', CRASH, uri, str(Path(__file__).parent), str(log), run],
                            timeout=120)
    assert worker.returncode == -signal.SIGKILL, worker.returncode
    (log.parent / 'exit.json').write_text(json.dumps({'code': -9}))
    trace = trace_id(log)

    def flush():
        with log.open('rb') as records:
            return subprocess.run([sys.executable, '-I', '-m', 'tl3_workers.tracing', 'flush', str(log)],
                                  stdin=records, timeout=60).returncode

    assert flush() == 0
    final = reader.until(trace, lambda info: info['state'] != 'IN_PROGRESS')
    spans = reader.spans(trace)
    assert final['state'] == 'ERROR', final['state']
    assert final['tags'].get('outcome') == 'crashed'
    assert final['tags'].get('failure_message') == 'worker exited with code -9'
    assert flush() == 0, 'a second flush, as after a controller restart'
    again = reader.spans(trace)
    assert sorted(str(span.get('span_id')) for span in again) == sorted(str(span.get('span_id')) for span in spans)
    return {'trace': trace, 'final': summary(final, spans), 'replayed_span_count': len(again)}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--tracking-uri', default='http://192.168.1.107:5033')
    args = parser.parse_args()
    os.environ['MLFLOW_DISABLE_TELEMETRY'] = 'true'
    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        responses = root / 'responses'
        responses.mkdir()
        reader = Reader(args.tracking_uri, responses)
        result = {'experiment': EXPERIMENT, 'live': live(args.tracking_uri, reader, root),
                  'crashed': crashed(args.tracking_uri, reader, root)}
    print(json.dumps(result, indent=2, sort_keys=True))
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
