"""One-pass evaluation of the existing classifier prompt and reply contract."""

import argparse
from contextlib import closing
import hashlib
from importlib.resources import files
import json
import os
from pathlib import Path
import sys
import tomllib

from langchain_core.messages import AIMessage, HumanMessage

from tl3_workers import runtime
from tl3_workers.contracts import read_json, validate
from tl3_workers.roles.classifier import ROLE, check_reply


KINDS = {'answer', 'change_request', 'complaint', 'pause', 'remark', 'resume', 'stop', 'unsure'}


def _validate_cases(cases):
    try:
        if not isinstance(cases, list) or not cases:
            raise ValueError('empty evaluation set')
        ids = set()
        for case in cases:
            if not isinstance(case['id'], str) or not case['id'] or case['id'] in ids:
                raise ValueError('invalid or duplicate case id')
            ids.add(case['id'])
            if not isinstance(case['category'], str) or not case['category']:
                raise ValueError('missing category')
            validate('classifier-input', case['inputs'])
            expected = case['expected']
            if expected['kind'] not in KINDS:
                raise ValueError('unknown label')
            # Labels omit free wording, but must still describe a valid reply.
            complete = dict(expected)
            for field in {'unsure': ['question'], 'complaint': ['summary'],
                          'change_request': ['new_text']}.get(expected['kind'], []):
                complete.setdefault(field, 'Evaluation label')
            if expected['kind'] == 'answer':
                for field in {'try_again': ['guidance'], 'change_request': ['new_text']}.get(
                    expected['answer'], []
                ):
                    complete.setdefault(field, 'Evaluation label')
            validate('classifier-reply', complete)
            check_reply(case['inputs'], complete)
    except Exception:
        raise ValueError('invalid evaluation set') from None


def load_cases(path=None):
    document = (Path(path) if path else files('tl3_workers').joinpath(
        'evaluation/classifier.json')).read_text()
    cases = read_json(document)
    _validate_cases(cases)
    return cases


def request_from_config(path):
    models = tomllib.loads(Path(path).read_text())['models']
    spec = models['comment_classifier'][0]
    return {
        'protocol': 1, 'operation': {'node': 0, 'generation': 0, 'kind': 'classifier'},
        'role': 'classifier',
        'model': {'base_url': models['base_url'], 'name': spec['name'],
                  'effort': spec.get('effort'), 'budget_tokens': spec['budget_tokens']},
        'key_file': models['key_file'], 'worktree': str(Path.cwd()),
    }


def _turn(agent, inputs):
    # Stop before any tool node: the classifier has no tools, and evaluation
    # must not repair a bad reply by spending a second model call.
    with closing(agent.stream(
        {'messages': [HumanMessage(content=json.dumps(inputs, ensure_ascii=False))]},
        stream_mode='updates',
    )) as updates:
        for update in updates:
            for result in update.values():
                if isinstance(result, dict):
                    for message in result.get('messages', []):
                        if isinstance(message, AIMessage):
                            return message
    raise ValueError('missing model turn')


def _usage(message):
    # LangChain maps the proxy's streamed usage fields to these names.
    usage = message.usage_metadata
    names = {'prompt_tokens': 'input_tokens', 'completion_tokens': 'output_tokens',
             'total_tokens': 'total_tokens'}
    if usage is None or any(type(usage.get(name)) is not int or usage[name] < 0
                            for name in names.values()):
        return None
    return {key: usage[value] for key, value in names.items()}


def _score(case, message):
    row = {**case, 'reply': None, 'usage': _usage(message), 'error': None,
           'correct': False, 'destructive': False}
    try:
        row['reply'] = read_json(message.text)
    except Exception:
        row['error'] = 'invalid_json'
        return row
    reply = row['reply']
    # Check destructive output before validation so an invalid answer cannot
    # hide a safety failure, for example by naming a question that is not open.
    if isinstance(reply, dict):
        destructive = reply.get('kind') in {'stop', 'change_request'} or (
            reply.get('kind') == 'answer' and reply.get('answer') in {'stop', 'change_request'})
        row['destructive'] = case['expected']['kind'] in {'remark', 'unsure'} and destructive
    try:
        if message.tool_calls or message.invalid_tool_calls:
            raise ValueError('classifier requested tools')
        validate('classifier-reply', reply)
        check_reply(case['inputs'], reply)
    except Exception:
        row['error'] = 'invalid_reply'
        return row
    row['correct'] = all(reply.get(key) == value for key, value in case['expected'].items())
    if not row['correct']:
        row['error'] = 'label_mismatch'
    return row


def evaluate(cases, request, *, model_factory=None):
    _validate_cases(cases)
    for case in cases:
        validate('request', {**request, 'inputs': case['inputs']})
    os.environ['LANGSMITH_TRACING'] = 'false'
    os.environ['LANGCHAIN_TRACING_V2'] = 'false'
    model = (model_factory or runtime.create_model)(request)
    if hasattr(model, 'stream_usage'):
        model.stream_usage = True
    agent = runtime.build_agent(model, ROLE)
    rows = []
    for case in cases:
        try:
            message = _turn(agent, case['inputs'])
        except Exception:
            # Provider exceptions may contain credentials. Never serialize them.
            row = {**case, 'reply': None, 'usage': None, 'error': 'model_call_failed',
                   'correct': False, 'destructive': False}
        else:
            row = _score(case, message)
        rows.append(row)
        print(f"classifier evaluation: {len(rows)}/{len(cases)}", file=sys.stderr, flush=True)
    by_kind = {}
    for row in rows:
        score = by_kind.setdefault(row['expected']['kind'], {'correct': 0, 'total': 0})
        score['total'] += 1
        score['correct'] += int(row['correct'])
    for score in by_kind.values():
        score['accuracy'] = score['correct'] / score['total']
    usage = {key: sum(row['usage'][key] for row in rows if row['usage'] is not None)
             for key in ['prompt_tokens', 'completion_tokens', 'total_tokens']}
    complete = all(row['usage'] is not None for row in rows)
    misses = [row for row in rows if not row['correct']]
    prompt = files('tl3_workers').joinpath('prompts', ROLE.prompt).read_bytes()
    return {
        'model': request['model'], 'cases_sha256': hashlib.sha256(
            json.dumps(cases, sort_keys=True, ensure_ascii=False).encode()).hexdigest(),
        'prompt_sha256': hashlib.sha256(prompt).hexdigest(),
        'by_kind': by_kind, 'usage': usage, 'usage_complete': complete,
        'destructive_misses': [row['id'] for row in rows if row['destructive']],
        'misses': misses, 'results': rows, 'passed': not misses and complete,
        'mlflow': 'not wired; retain this report for later import',
    }


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--config', required=True, type=Path)
    parser.add_argument('--cases', type=Path, help='defaults to the packaged labelled set')
    parser.add_argument('--output', type=Path, help='also save the JSON report here')
    args = parser.parse_args(argv)
    try:
        cases = load_cases(args.cases)
        report = evaluate(cases, request_from_config(args.config))
        document = json.dumps(report, ensure_ascii=False, indent=2, allow_nan=False) + '\n'
        if args.output:
            args.output.write_text(document)
        print(document, end='')
        return 0 if report['passed'] else 1
    except Exception:
        print('classifier evaluation failed; details suppressed', file=sys.stderr)
        return 2


if __name__ == '__main__':
    raise SystemExit(main())
