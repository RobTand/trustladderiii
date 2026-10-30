from types import SimpleNamespace

from tl3_workers.quota import reset_ms


def response(headers, status=429):
    return SimpleNamespace(response=SimpleNamespace(status_code=status, headers=headers))


def test_provider_reset_time_is_required():
    assert reset_ms(response({"retry-after": "5"}), 1000) == 1005000
    assert reset_ms(response({"x-ratelimit-reset": "1008"}), 1000) == 1008000
    for headers in [{}, {"retry-after": "NaN"}, {"retry-after": "-1"},
                    {"retry-after": "bad"}, {"x-ratelimit-reset": "999"}]:
        assert reset_ms(response(headers), 1000) is None
    assert reset_ms(response({"retry-after": "5"}, 500), 1000) is None
    assert reset_ms(RuntimeError("secret exception text"), 1000) is None
