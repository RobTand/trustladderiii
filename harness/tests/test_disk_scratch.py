from pathlib import Path

from conftest import flux_context


def test_each_container_gets_a_private_child_of_the_job_leaf(monkeypatch):
    from tl3_harness import scratch

    # PB-REMOVAL: the Rust Flux launch context names the only job scratch leaf.
    leaf = Path(flux_context(monkeypatch)['scratch']['leaf'])
    first, second = scratch.directory(), scratch.directory()
    assert first != second
    assert first.parent == second.parent == leaf
    assert first.stat().st_mode & 0o777 == second.stat().st_mode & 0o777 == 0o700
