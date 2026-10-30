#!/usr/bin/env python3
"""Live MLflow deployment check. Submit through PrismaBuild on dl380g10."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import tomllib
import uuid
from unittest.mock import patch


def store_check(trace_id):
    import psycopg

    root = Path.home() / '.local/share/tl3'
    # This runtime-only read never enters the submitting session or a snapshot.
    password = (Path.home() / '.config/tl3/secrets/postgres-password').read_text().strip()
    with psycopg.connect(host=str(root / 'postgres/socket'), port=5543, user='tl3',
                         password=password, dbname='tl3_mlflow', connect_timeout=5) as database:
        assert database.execute('SELECT current_database()').fetchone() == ('tl3_mlflow',)
        assert database.execute('SELECT count(*) FROM trace_info WHERE request_id = %s',
                                (trace_id,)).fetchone()[0] == 1
        assert database.execute('SELECT count(*) FROM spans WHERE trace_id = %s',
                                (trace_id,)).fetchone()[0] > 1
    print('OK Postgres tl3_mlflow: trace and spans persisted over Unix socket')


def live_check():
    import httpx
    from langchain_core.language_models.fake_chat_models import FakeMessagesListChatModel
    from langchain_core.messages import AIMessage
    from mlflow.tracing.client import TracingClient
    from tl3_workers import runtime

    assert os.environ.get('PRISMABUILD_ACTION_KEY'), 'submit this check through PrismaBuild'
    config = tomllib.loads((Path.home() / '.config/tl3/mlflow.toml').read_text())['mlflow']
    with httpx.Client(base_url=config['tracking_uri'], timeout=3, trust_env=False) as client:
        def health():
            deadline = time.monotonic() + 60
            while True:
                try:
                    response = client.get('/health')
                    response.raise_for_status()
                    return
                except httpx.HTTPError:
                    if time.monotonic() >= deadline:
                        raise
                    time.sleep(0.2)

        health()
        assert client.get('/version').text == '3.17.2'
        print('OK live MLflow 3.17.2 HTTP health')
        run_id = uuid.uuid4().hex
        config['experiment'] = 'tl3-deploy-check'
        reply = {'kind': 'brief', 'brief': 'fixture-secret-value',
                 'goals': [{'description': 'Return hello.', 'verification': 'test'}], 'memory_ids': []}
        model = FakeMessagesListChatModel(responses=[AIMessage(
            content=json.dumps(reply), usage_metadata={'input_tokens': 11, 'output_tokens': 7,
                                                     'total_tokens': 18})])
        children = []
        popen = subprocess.Popen

        def launch(*args, **kwargs):
            child = popen(*args, **kwargs)
            children.append(child)
            return child

        with tempfile.TemporaryDirectory() as directory:
            request = {'protocol': 1, 'operation': {'node': 101, 'generation': 0, 'kind': 'ingest'},
                       'role': 'ingest', 'model': {'base_url': 'http://model.invalid',
                       'name': 'fake-model', 'effort': 'high', 'budget_tokens': 1000},
                       'key_file': directory + '/unused', 'worktree': directory,
                       'inputs': {'issue': {'number': 101, 'title': 'Fixture', 'body': 'fixture-secret-value',
                       'author': 'fixture', 'comments': []}, 'repo': {'readme': '', 'file_list': []}},
                       'mlflow': config, 'trace': {'repo': 'RobTand/tl3-sandbox', 'issue': 101,
                       'request': run_id, 'attempt': 1, 'round': 0}}
            events = []
            with patch.object(runtime, 'create_model', return_value=model), patch.object(subprocess, 'Popen', launch):
                runtime.run(request, events.append)
                for child in children:
                    assert child.wait(timeout=15) == 0
            assert events[-1]['reply'] == reply
        experiment = client.get('/api/2.0/mlflow/experiments/get-by-name',
                                params={'experiment_name': config['experiment']}).json()['experiment']['experiment_id']
        traces = TracingClient(tracking_uri=config['tracking_uri'])
        found = traces.search_traces(experiment_ids=[experiment], filter_string=f"tags.request = '{run_id}'")
        assert len(found) == 1
        trace = found[0]
        assert trace.info.tags['outcome'] == 'succeeded'
        assert trace.info.tags['total_tokens'] == '18'
        assert trace.info.tags['role'] == 'ingest'
        assert 'fixture-secret-value' not in json.dumps(trace.to_dict())
        trace_id = trace.info.trace_id
        print(f'OK live tagged fake-model trace {trace_id}, tokens=18, no fixture secret')
        before = subprocess.check_output(['systemctl', '--user', 'show', 'tl3-mlflow', '-p', 'MainPID', '--value'], text=True).strip()
        subprocess.run(['systemctl', '--user', 'restart', 'tl3-mlflow.service'], check=True)
        health()
        after = subprocess.check_output(['systemctl', '--user', 'show', 'tl3-mlflow', '-p', 'MainPID', '--value'], text=True).strip()
        assert before != after
        assert traces.get_trace(trace_id).info.tags['request'] == run_id
        print(f'OK MLflow restart: PID {before} -> {after}, same trace retained')
        root = Path.home() / '.local/share/tl3'
        checked = subprocess.run([root / 'mlflow/.venv/bin/python', __file__, '--store', trace_id],
                                 capture_output=True, text=True)
        assert checked.returncode == 0, 'Postgres persistence check failed; details suppressed'
        print(checked.stdout.strip())


if __name__ == '__main__':
    if len(sys.argv) == 3 and sys.argv[1] == '--store':
        try:
            store_check(sys.argv[2])
        except Exception:
            print('Postgres persistence check failed; details suppressed', file=sys.stderr)
            raise SystemExit(1)
    else:
        live_check()
