"""Run an isolated sandbox ingest and read its scratch MLflow trace over REST.

Submit through PrismaBuild. Only runtime.create_model loads the model key.
--recorded exercises the real graph with a recorded LangChain model instead.
No installed worker, service, database or run directory is changed.
"""
import argparse
from contextlib import nullcontext
import json
import os
from pathlib import Path
import subprocess
import tempfile
from unittest.mock import patch
from uuid import uuid4

import httpx

from tl3_workers import runtime


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--tracking-uri', default='http://192.168.1.107:5033')
    parser.add_argument('--base-url', default='http://192.168.1.120:8317/v1')
    parser.add_argument('--model', default='claude-sonnet-5-5')
    parser.add_argument('--key-file', default='/home/rob/.config/tl3/secrets/model-key')
    parser.add_argument('--recorded', action='store_true')
    args = parser.parse_args()
    assert os.environ.get('PRISMABUILD_ACTION_KEY'), 'submit through PrismaBuild'
    os.environ['LANGSMITH_TRACING'] = 'false'
    os.environ['LANGCHAIN_TRACING_V2'] = 'false'
    request_id = uuid4().hex
    children = []
    popen = subprocess.Popen

    def launch(*argv, **kwargs):
        child = popen(*argv, **kwargs)
        children.append(child)
        return child

    with tempfile.TemporaryDirectory(prefix='tl3-trace-trial-') as directory:
        root = Path(directory)
        checkout = root / 'sandbox'
        subprocess.run(['git', 'clone', '--quiet', '--depth=1',
                        'https://github.com/RobTand/tl3-sandbox.git', str(checkout)], check=True)
        paths = subprocess.check_output(['git', '-C', str(checkout), 'ls-files'], text=True).splitlines()
        readme = next((path for path in paths if path.lower().startswith('readme')), paths[0])
        title = 'Explain the purpose of this sandbox in a brief'
        request = {
            'protocol': 1, 'operation': {'node': 296, 'generation': 0, 'kind': 'ingest'},
            'role': 'ingest', 'worktree': str(checkout), 'key_file': args.key_file,
            'model': {'base_url': args.base_url, 'name': args.model, 'effort': 'high',
                      'budget_tokens': 4000},
            'inputs': {'issue': {'number': 296, 'title': title,
                'body': f'Read {readme} with the read tool, then return a brief explaining this '
                        'repository. This is an observation trial; no changes are requested.',
                'author': 'fixture', 'comments': []},
                'repo': {'readme': '', 'file_list': paths}},
            'mlflow': {'tracking_uri': args.tracking_uri, 'experiment': 'tl3-dev-296'},
            'trace': {'repo': 'RobTand/tl3-sandbox', 'issue': 296, 'request': request_id,
                      'attempt': 1, 'round': 0},
        }
        model_context = nullcontext()
        if args.recorded:
            from langchain_core.language_models.fake_chat_models import FakeMessagesListChatModel
            from langchain_core.messages import AIMessage
            reply = {'kind': 'brief', 'brief': 'The sandbox exercises TL3 changes.',
                     'goals': [{'description': title, 'verification': 'read the README'}], 'memory_ids': []}
            counts = {'input_tokens': 11, 'output_tokens': 7, 'total_tokens': 18,
                      'input_token_details': {'cache_read': 8, 'cache_creation': 2}}
            class RecordedModel(FakeMessagesListChatModel):
                def bind_tools(self, tools, **kwargs):
                    return self
            model = RecordedModel(responses=[
                AIMessage(content='', tool_calls=[{'name': 'read', 'args': {'path': readme},
                                                  'id': 'trial-read'}], usage_metadata=counts),
                AIMessage(content=json.dumps(reply), usage_metadata=counts),
            ])
            model_context = patch.object(runtime, 'create_model', return_value=model)
        print('trial: worker call', flush=True)
        with model_context, patch.object(subprocess, 'Popen', launch):
            events = []
            runtime.run(request, events.append)
            assert events[-1]['event'] == 'reply'
            # Only this evidence collector waits. runtime.run has already returned.
            for child in children:
                assert child.wait(timeout=15) == 0, 'export child failed'

    print('trial: REST read-back', flush=True)
    from google.protobuf.json_format import ParseDict
    from mlflow.entities import Trace
    from mlflow.protos.service_pb2 import BatchGetTraces, SearchTracesV3
    with httpx.Client(base_url=args.tracking_uri, trust_env=False, timeout=15) as client:
        response = client.get('/api/2.0/mlflow/experiments/get-by-name',
                              params={'experiment_name': 'tl3-dev-296'})
        response.raise_for_status()
        experiment = response.json()['experiment']['experiment_id']
        response = client.post('/api/3.0/mlflow/traces/search', json={
            'locations': [{'mlflow_experiment': {'experiment_id': experiment}}],
            'filter': f"tags.request = '{request_id}'", 'max_results': 1})
        response.raise_for_status()
        found = ParseDict(response.json(), SearchTracesV3.Response())
        assert len(found.traces) == 1, 'scratch trace not found'
        trace_id = found.traces[0].trace_id
        response = client.post('/api/3.0/mlflow/traces/batchGet', json={'trace_ids': [trace_id]})
        response.raise_for_status()
        fetched = ParseDict(response.json(), BatchGetTraces.Response())
        trace = Trace.from_proto(fetched.traces[0])
    spans = trace.data.spans
    root = next(span for span in spans if span.parent_id is None)
    models = [span for span in spans if span.span_type == 'CHAT_MODEL']
    tools = [span for span in spans if span.span_type == 'TOOL']
    assert root.inputs and root.outputs and models
    assert trace.info.request_preview and trace.info.response_preview
    assert tools, 'trial must exercise a tool call'
    for span in models + tools:
        assert span.inputs is not None and span.outputs is not None
    for span in models:
        usage = span.get_attribute('mlflow.chat.tokenUsage')
        assert all(key in usage for key in ['input_tokens', 'output_tokens', 'total_tokens'])
    print(json.dumps({
        'mode': 'recorded' if args.recorded else 'real_proxy', 'trace_id': trace_id,
        'experiment': experiment, 'span_count': len(spans),
        'request_preview': trace.info.request_preview, 'response_preview': trace.info.response_preview,
        'root_inputs': root.inputs, 'root_outputs': root.outputs, 'root_tags': trace.info.tags,
        'spans': [{'name': span.name, 'type': span.span_type, 'inputs': span.inputs,
                   'outputs': span.outputs, 'usage': span.get_attribute('mlflow.chat.tokenUsage'),
                   'raw_usage_chunks': span.get_attribute('tl3.proxy.usage_chunks')}
                  for span in models + tools],
    }, ensure_ascii=False), flush=True)


if __name__ == '__main__':
    try:
        main()
    except Exception as error:
        # Provider errors can contain headers or credentials. Report only the class.
        print('trace trial failed: ' + type(error).__name__, flush=True)
        raise SystemExit(1)
