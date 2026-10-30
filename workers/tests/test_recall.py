import asyncio
import json

import httpx
import pytest

from tl3_workers import recall
from tl3_workers.tools import Worktree, build_tools


def client(monkeypatch, handler):
    original = httpx.AsyncClient
    monkeypatch.setattr(httpx, 'AsyncClient', lambda **kwargs: original(transport=httpx.MockTransport(handler), **kwargs))


def test_recall_uses_only_configured_bank(monkeypatch, tmp_path):
    requests = []

    def handler(request):
        requests.append(request)
        return httpx.Response(200, json={'results': [{'id': 'one', 'text': 'Keep greet().'}]})

    client(monkeypatch, handler)
    with Worktree(tmp_path) as tree:
        tree.hindsight_endpoint = 'http://127.0.0.1:8890'
        tree.hindsight_bank = 'tl3 project/one'
        tool, = build_tools(tree, {'recall': 'Read project memories.'})
        assert set(tool.args) == {'query'}
        result = tool.invoke({'query': 'greeting decisions'})
    assert len(requests) == 1
    assert requests[0].method == 'POST'
    assert str(requests[0].url) == 'http://127.0.0.1:8890/v1/default/banks/tl3%20project%2Fone/memories/recall'
    assert json.loads(requests[0].content) == {'query': 'greeting decisions', 'budget': 'low', 'max_tokens': 1024}
    assert result == {'results': [{'id': 'one', 'text': 'Keep greet().'}]}


@pytest.mark.parametrize('count,characters,expected_count,expected_characters', [
    (30, 10, 20, 10),
    (1, 5000, 1, 4096),
    (10, 4096, 3, 4096),
    (10, 500, 2, 500),
])
def test_each_output_cap_is_reachable(monkeypatch, count, characters, expected_count, expected_characters):
    text = ('\U0001f642' if characters == 500 else 'x') * characters
    payload = {'results': [{'id': str(i), 'text': text} for i in range(count)]}
    raw = json.dumps(payload, ensure_ascii=False).encode()
    if characters == 500:
        # Keep the wire body under 64 KiB while exercising escaped output bytes.
        payload['results'] = payload['results'][:7]
        raw = json.dumps(payload, ensure_ascii=False).encode()
    assert len(raw) <= 65536
    client(monkeypatch, lambda request: httpx.Response(200, content=raw))
    result = recall.recall('http://127.0.0.1:8890', 'tl3', 'greeting')
    assert 'error' not in result
    assert len(result['results']) == expected_count
    assert all(len(item['text']) == expected_characters for item in result['results'])
    assert len(json.dumps(result['results']).encode()) <= 15000


def test_raw_response_cap(monkeypatch):
    client(monkeypatch, lambda request: httpx.Response(200, content=b'x' * 65537))
    assert 'error' in recall.recall('http://127.0.0.1:8890', 'tl3', 'greeting')


def test_total_deadline_covers_a_trickling_response(monkeypatch):
    closed = []
    reads = []

    class Trickle(httpx.AsyncByteStream):
        async def __aiter__(self):
            yield b'{"results":['
            # Every read completes, but the response never ends.
            while True:
                await asyncio.sleep(0)
                reads.append(True)
                yield b' '

        async def aclose(self):
            closed.append(True)

    monkeypatch.setattr(recall, 'DEADLINE_SECONDS', 0.01)
    client(monkeypatch, lambda request: httpx.Response(200, stream=Trickle()))
    result = recall.recall('http://127.0.0.1:8890', 'tl3', 'greeting')
    assert result['results'] == [] and 'error' in result
    assert reads and closed


@pytest.mark.parametrize('response', [
    httpx.Response(200, json={'results': [{'id': 'one', 'text': 'Keep greet().'}]}),
    httpx.Response(503, text='internal details'),
    httpx.Response(302, headers={'location': 'http://untrusted.invalid'}),
    httpx.Response(200, json={'wrong': []}),
    httpx.Response(200, content=b'not json'),
])
def test_fake_hindsight_never_needs_network(monkeypatch, response):
    client(monkeypatch, lambda request: response)
    result = recall.recall('http://127.0.0.1:8890', 'tl3', 'greeting')
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
