import os

import pytest


@pytest.fixture(autouse=True)
def disk_sync(request, monkeypatch):
    if request.node.get_closest_marker('durability'):
        return
    # Content and verdict tests do not need shared-disk durability barriers.
    monkeypatch.setattr(os, 'fsync', lambda descriptor: None)
    monkeypatch.setattr(os, 'fdatasync', lambda descriptor: None)
