import json

import httpx
import pytest

from tl3_workers import recall
from tl3_workers.tools import Worktree, build_tools


def client(monkeypatch, handler):
    original = httpx.Client
    monkeypatch.setattr(httpx, 'Client', lambda **kwargs: original(transport=httpx.MockTransport(handler), **kwargs))


def test_recall_uses_only_configured_bank_and_bounds_output(monkeypatch, tmp_path):
    requests = []

    def handler(request):
        requests.append(request)
        return httpx.Response(200, json={'results': [{'id': str(i), 'text': 'hello' * 900} for i in range(30)]})

    client(monkeypatch, handler)
    with Worktree(tmp_path) as tree:
        tree.hindsight_endpoint = 'http://127.0.0.1:8890'
        tool, = build_tools(tree, {'recall': 'Read project memories.'})
        assert set(tool.args) == {'query'}
        result = tool.invoke({'query': 'greeting decisions'})
    assert len(requests) == 1
    assert requests[0].method == 'POST'
    assert str(requests[0].url) == 'http://127.0.0.1:8890/v1/default/banks/default/memories/recall'
    assert json.loads(requests[0].content) == {'query': 'greeting decisions', 'budget': 'low', 'max_tokens': 1024}
    # The response above is too large and must produce no memory evidence.
    assert result['results'] == [] and 'error' in result


@pytest.mark.parametrize('response', [
    httpx.Response(200, json={'results': [{'id': 'one', 'text': 'Keep greet().'}]}),
    httpx.Response(503, text='internal details'),
    httpx.Response(302, headers={'location': 'http://untrusted.invalid'}),
    httpx.Response(200, json={'wrong': []}),
    httpx.Response(200, content=b'not json'),
])
def test_fake_hindsight_never_needs_network(monkeypatch, response):
    client(monkeypatch, lambda request: response)
    result = recall.recall('http://127.0.0.1:8890', 'greeting')
    if response.status_code == 200 and 'results' in response.text:
        assert result == {'results': [{'id': 'one', 'text': 'Keep greet().'}]}
    else:
        assert result['results'] == [] and 'error' in result
        assert 'internal details' not in json.dumps(result)


def test_recall_has_no_write_or_endpoint_argument(tmp_path):
    with Worktree(tmp_path) as tree:
        tool, = build_tools(tree, {'recall': 'Recall.'})
        assert 'error' in tool.invoke({'query': 'greeting'})
        assert 'nonempty recall query' in tool.invoke({'query': ' '})
