"""Durable worker completion facts, independent of trace export."""
from contextlib import contextmanager


@contextmanager
def observe(request):
    raise NotImplementedError("worker boundary facts")
    yield
