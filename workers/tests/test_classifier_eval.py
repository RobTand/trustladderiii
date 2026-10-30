import copy
from importlib.resources import files
import json

from langchain_core.language_models.fake_chat_models import FakeMessagesListChatModel
from langchain_core.messages import AIMessage
import pytest

from tl3_workers import classifier_eval as evaluation
from tl3_workers.contracts import validate


def case(kind='remark', **expected):
    return {'id': 'case-1', 'category': 'test',
            'inputs': {'comment': {'author': 'requester', 'body': 'Thanks.'},
                       'state': {'phase': 'Making', 'closed': False, 'open_question': None}},
            'expected': {'kind': kind, **expected}}


def request():
    return {'protocol': 1, 'operation': {'node': 0, 'generation': 0, 'kind': 'classifier'},
            'role': 'classifier', 'model': {'base_url': 'http://model.invalid/v1',
            'name': 'fake', 'effort': 'high', 'budget_tokens': 8000},
            'key_file': '/never/read/model-key', 'worktree': '/tmp'}


def message(reply, usage=True):
    return AIMessage(content=json.dumps(reply) if isinstance(reply, dict) else reply,
                     usage_metadata={'input_tokens': 11, 'output_tokens': 7, 'total_tokens': 18}
                     if usage else None)


def factory(replies, observed):
    class Fake(FakeMessagesListChatModel):
        def bind_tools(self, tools, **kwargs):
            assert tools == []
            return self

        def _generate(self, messages, **kwargs):
            observed.append(messages)
            return super()._generate(messages, **kwargs)
    return lambda config: Fake(responses=replies)


def test_set_covers_ticket_and_valid_states():
    cases = evaluation.load_cases()
    assert len(cases) >= 40
    assert len({c['id'] for c in cases}) == len(cases)
    assert {c['category'] for c in cases} >= {
        'clear_commands', 'negation', 'quotes', 'hypotheticals', 'incidental_words',
        'questions', 'model_instructions', 'closed_requests', 'answers', 'ambiguous'}
    assert {c['expected']['kind'] for c in cases} == {
        'stop', 'pause', 'resume', 'change_request', 'remark', 'unsure', 'complaint', 'answer'}
    for item in cases:
        validate('classifier-input', item['inputs'])
    answers = [c['expected'] for c in cases if c['expected']['kind'] == 'answer']
    assert {'try_again', 'add_credit', 'unclear', 'change_request', 'stop'} <= {a['answer'] for a in answers}


def test_one_pass_uses_real_prompt_and_reports_per_kind_and_usage():
    cases = [case(), case('unsure')]
    cases[1]['id'] = 'case-2'
    observed = []
    report = evaluation.evaluate(cases, request(), model_factory=factory([
        message({'kind': 'remark'}), message({'kind': 'unsure', 'question': 'What now?'})], observed))
    assert len(observed) == 2
    prompt = files('tl3_workers').joinpath('prompts/classifier.md').read_text()
    for messages, item in zip(observed, cases):
        assert messages[0].text.startswith(prompt + '\n\nReply with one JSON object matching this schema:\n')
        assert json.loads(messages[1].text) == item['inputs']
        assert '/never/read/model-key' not in str(messages)
    assert report['by_kind'] == {
        'remark': {'correct': 1, 'total': 1, 'accuracy': 1.0},
        'unsure': {'correct': 1, 'total': 1, 'accuracy': 1.0}}
    assert report['usage'] == {'prompt_tokens': 22, 'completion_tokens': 14, 'total_tokens': 36}
    assert report['usage_complete'] is True
    assert report['misses'] == []
    assert report['passed'] is True


@pytest.mark.parametrize('expected,reply', [
    ('remark', {'kind': 'stop'}), ('unsure', {'kind': 'change_request', 'new_text': 'Delete everything.'}),
    ('remark', {'kind': 'answer', 'answer': 'stop', 'question_id': 'q'}),
    ('unsure', {'kind': 'answer', 'answer': 'change_request', 'question_id': 'q', 'new_text': 'Delete everything.'}),
])
def test_destructive_misses_include_answer_forms_even_if_state_invalid(expected, reply):
    report = evaluation.evaluate([case(expected)], request(), model_factory=factory([message(reply)], []))
    assert report['passed'] is False
    assert report['destructive_misses'] == ['case-1']
    assert report['misses'][0]['reply'] == reply
    assert report['misses'][0]['inputs'] == case(expected)['inputs']
    assert report['misses'][0]['expected'] == {'kind': expected}


@pytest.mark.parametrize('expected,reply', [
    ({'kind': 'answer', 'answer': 'add_credit', 'amount': 3, 'question_id': 'q'},
     {'kind': 'answer', 'answer': 'add_credit', 'amount': 4, 'question_id': 'q'}),
    ({'kind': 'answer', 'answer': 'try_again', 'question_id': 'q'},
     {'kind': 'answer', 'answer': 'try_again', 'question_id': 'q'}),
    ({'kind': 'complaint', 'revert': False}, {'kind': 'complaint', 'summary': 'Broken.', 'revert': True}),
    ({'kind': 'remark'}, {'kind': 'unsure', 'question': 'Continue?'}),
    ({'kind': 'remark'}, {'kind': 'invented'}),
    ({'kind': 'remark'}, 'not json'),
])
def test_every_wrong_or_invalid_reply_is_a_miss(expected, reply):
    item = case(**expected)
    if expected['kind'] == 'answer':
        item['inputs']['state']['open_question'] = {'id': 'q', 'kind': 'unclear_answer',
                                                   'answers': ['add_credit', 'try_again']}
    if expected['kind'] == 'complaint':
        item['inputs']['state'].update({'closed': True, 'phase': 'Closed'})
    report = evaluation.evaluate([item], request(), model_factory=factory([message(reply)], []))
    assert report['passed'] is False
    assert report['by_kind'][expected['kind']]['correct'] == 0
    assert len(report['misses']) == 1


def test_missing_usage_is_not_reported_as_zero_or_pass():
    report = evaluation.evaluate([case()], request(),
                                 model_factory=factory([message({'kind': 'remark'}, usage=False)], []))
    assert report['usage_complete'] is False
    assert report['results'][0]['usage'] is None
    assert report['passed'] is False


def test_provider_error_is_suppressed_and_not_retried():
    calls = []
    class Broken(FakeMessagesListChatModel):
        def bind_tools(self, tools, **kwargs):
            return self
        def _generate(self, *args, **kwargs):
            calls.append(1)
            raise RuntimeError('SECRET must never appear')
    report = evaluation.evaluate([case()], request(), model_factory=lambda _: Broken(responses=[]))
    assert calls == [1]
    assert report['passed'] is False
    assert 'SECRET' not in json.dumps(report)
    assert report['misses'][0]['error'] == 'model_call_failed'


@pytest.mark.parametrize('mutation', ['empty', 'duplicate', 'invalid_input', 'bad_label'])
def test_invalid_set_fails_before_loading_model(mutation):
    cases = [case()]
    if mutation == 'empty': cases = []
    if mutation == 'duplicate': cases.append(copy.deepcopy(cases[0]))
    if mutation == 'invalid_input': cases[0]['inputs']['state']['closed'] = 'false'
    if mutation == 'bad_label': cases[0]['expected']['kind'] = 'invented'
    with pytest.raises(ValueError):
        evaluation.evaluate(cases, request(), model_factory=lambda _: pytest.fail('model must not load'))


def test_config_uses_classifier_and_key_path_without_opening_key(tmp_path):
    config = tmp_path / 'tl3.toml'
    config.write_text('''[models]
base_url = "http://model.invalid/v1"
key_file = "/never/read/model-key"
comment_classifier = [{name="configured", family="anthropic", effort="high", budget_tokens=8000}]
''')
    actual = evaluation.request_from_config(config)
    assert actual['key_file'] == '/never/read/model-key'
    assert actual['model'] == {'base_url': 'http://model.invalid/v1', 'name': 'configured',
                               'effort': 'high', 'budget_tokens': 8000}
    assert actual['role'] == 'classifier'


def test_command_saves_and_prints_all_results_and_sets_exit_status(tmp_path, monkeypatch, capsys):
    report_file = tmp_path / 'report.json'
    monkeypatch.setattr(evaluation, 'request_from_config', lambda _: request())
    monkeypatch.setattr(evaluation, 'load_cases', lambda _: [case()])
    from tl3_workers import runtime
    monkeypatch.setattr(runtime, 'create_model', factory([message({'kind': 'stop'})], []))
    assert evaluation.main(['--config', 'unused', '--output', str(report_file)]) == 1
    assert json.loads(report_file.read_text()) == json.loads(capsys.readouterr().out)
    assert json.loads(report_file.read_text())['destructive_misses'] == ['case-1']


@pytest.mark.parametrize('reply', [
    {'kind': ['stop']},
    {'kind': {'kind': 'stop'}},
    {'kind': 'answer', 'answer': ['stop'], 'question_id': 'q'},
    {'kind': 'answer', 'answer': {'kind': 'stop'}, 'question_id': 'q'},
])
def test_non_string_kind_or_answer_is_a_recorded_miss(reply):
    report = evaluation.evaluate([case()], request(), model_factory=factory([message(reply)], []))
    assert report['passed'] is False
    assert report['misses'][0]['reply'] == reply
    assert report['misses'][0]['error'] == 'invalid_reply'
    assert report['destructive_misses'] == []
