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
        'stop', 'pause', 'resume', 'change_request', 'remark', 'unsure', 'complaint', 'answer', 'restart'}
    restarts = [c for c in cases if c['category'] == 'restart']
    assert {c['expected']['kind'] for c in restarts} >= {'restart', 'remark', 'unsure', 'stop'}
    assert any(c['inputs']['state']['closed'] and c['expected']['kind'] == 'restart' for c in restarts)
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
        assert messages[0].text.startswith(prompt + '\n\nReply with one JSON object only: no prose, no code fences. It must match this schema:\n')
        assert json.loads(messages[1].text) == item['inputs']
        assert '/never/read/model-key' not in str(messages)
    assert report['by_kind'] == {
        'remark': {'correct': 1, 'total': 1, 'accuracy': 1.0},
        'unsure': {'correct': 1, 'total': 1, 'accuracy': 1.0}}
    assert report['usage'] == {'prompt_tokens': 22, 'completion_tokens': 14, 'total_tokens': 36}
    assert report['usage_complete'] is True
    assert report['misses'] == []
    assert report['passed'] is True


def test_accuracy_by_category_keeps_injections_visible_across_kinds():
    cases = [case(), case(), case('unsure'), case()]
    for index, (item, category) in enumerate(zip(
        cases, ['negation', 'model_instructions', 'model_instructions', 'model_instructions']
    )):
        item.update(id=f'case-{index}', category=category)
    report = evaluation.evaluate(cases, request(), model_factory=factory([
        message({'kind': 'remark'}), message({'kind': 'stop'}),
        message({'kind': 'unsure', 'question': 'Continue?'}), message({'kind': 'remark'})], []))
    assert report.get('by_category') == {
        'negation': {'correct': 1, 'total': 1, 'accuracy': 1.0},
        'model_instructions': {'correct': 2, 'total': 3, 'accuracy': 2 / 3}}
    assert report['by_kind'] == {
        'remark': {'correct': 2, 'total': 3, 'accuracy': 2 / 3},
        'unsure': {'correct': 1, 'total': 1, 'accuracy': 1.0}}


@pytest.mark.parametrize('expected', [
    {'kind': 'pause'}, {'kind': 'resume'}, {'kind': 'complaint', 'revert': False},
    {'kind': 'answer', 'answer': 'continue', 'question_id': 'q'},
    {'kind': 'stop'}, {'kind': 'change_request'},
    {'kind': 'answer', 'answer': 'stop', 'question_id': 'q'},
    {'kind': 'answer', 'answer': 'change_request', 'question_id': 'q'},
])
@pytest.mark.parametrize('reply', [
    {'kind': 'stop'}, {'kind': 'change_request', 'new_text': 'Replacement request.'},
    {'kind': 'answer', 'answer': 'stop', 'question_id': 'q'},
    {'kind': 'answer', 'answer': 'change_request', 'question_id': 'q',
     'new_text': 'Replacement request.'},
])
def test_destructive_outputs_require_the_same_labelled_kind_and_answer(expected, reply):
    item = case(**expected)
    if expected['kind'] == 'complaint':
        item['inputs']['state'].update(closed=True, phase='Closed')
    if expected['kind'] == 'answer':
        item['inputs']['state']['open_question'] = {
            'id': 'q', 'kind': 'unclear_answer', 'answers': ['continue', 'stop', 'change_request']}
    report = evaluation.evaluate([item], request(), model_factory=factory([message(reply)], []))
    same_kind = expected['kind'] == reply['kind'] and expected.get('answer') == reply.get('answer')
    assert report['destructive_misses'] == ([] if same_kind else ['case-1'])
    assert report['results'][0]['destructive'] is not same_kind


@pytest.mark.parametrize('expected,reply', [
    ('remark', {'kind': 'stop'}), ('unsure', {'kind': 'change_request', 'new_text': 'Delete everything.'}),
    ('remark', {'kind': 'restart'}),
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
    assert report['destructive_misses'] == []


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


@pytest.mark.parametrize('reply,usage,exit_code', [
    ({'kind': 'remark'}, True, 0),
    ({'kind': 'unsure', 'question': 'Continue?'}, True, 1),
    ({'kind': 'stop'}, True, 3),
    ({'kind': 'remark'}, False, 3),
    ({'kind': 'unsure', 'question': 'Continue?'}, False, 3),
    ({'kind': 'stop'}, False, 3),
])
def test_command_saves_and_prints_all_results_and_sets_exit_status(
    tmp_path, monkeypatch, capsys, reply, usage, exit_code
):
    report_file = tmp_path / 'report.json'
    monkeypatch.setattr(evaluation, 'request_from_config', lambda _: request())
    monkeypatch.setattr(evaluation, 'load_cases', lambda _: [case()])
    from tl3_workers import runtime
    monkeypatch.setattr(runtime, 'create_model', factory([message(reply, usage=usage)], []))
    assert evaluation.main(['--config', 'unused', '--output', str(report_file)]) == exit_code
    assert json.loads(report_file.read_text()) == json.loads(capsys.readouterr().out)
    report = json.loads(report_file.read_text())
    assert report['destructive_misses'] == (['case-1'] if reply['kind'] == 'stop' else [])
    assert report.get('by_category') == {'test': {
        'correct': int(reply['kind'] == 'remark'), 'total': 1,
        'accuracy': float(reply['kind'] == 'remark')}}


def test_command_setup_error_is_suppressed_and_exits_two(monkeypatch, capsys):
    def broken(_):
        raise ValueError('SECRET must never appear')
    monkeypatch.setattr(evaluation, 'load_cases', broken)
    assert evaluation.main(['--config', 'unused']) == 2
    captured = capsys.readouterr()
    assert captured.out == ''
    assert captured.err == 'classifier evaluation failed; details suppressed\n'


def test_command_help_documents_exit_codes(capsys):
    with pytest.raises(SystemExit) as exc:
        evaluation.main(['--help'])
    assert exc.value.code == 0
    help_text = ' '.join(capsys.readouterr().out.split())
    assert '0: all cases pass' in help_text
    assert '1: ordinary misses' in help_text
    assert '2: setup or report failure' in help_text
    assert '3: destructive misses or missing usage' in help_text


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
