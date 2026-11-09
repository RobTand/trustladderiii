import os

import pytest


@pytest.fixture(autouse=True)
def disk_sync(request, monkeypatch):
    if request.node.get_closest_marker('durability'):
        return
    # Content and verdict tests do not need shared-disk durability barriers.
    monkeypatch.setattr(os, 'fsync', lambda descriptor: None)
    monkeypatch.setattr(os, 'fdatasync', lambda descriptor: None)


def flux_context(monkeypatch, *, no_progress_seconds=120, **extra):
    """PB-REMOVAL: the harness runs only inside a Rust-launched Flux job.

    Supply the launch context that job would carry: run identity, a private job
    scratch leaf, the progress journal and its no-progress allowance. The leaf
    lives outside every fixture repository so it never becomes a changed file.
    """
    import tempfile
    from pathlib import Path

    from tl3_harness import runtime

    root = Path(tempfile.mkdtemp(prefix='tl3-flux-fixture-'))
    leaf = root / 'scratch'
    leaf.mkdir(mode=0o700)
    output = root / 'output'
    output.mkdir()
    context = {
        'run': {'backend': 'flux', 'uri': 'local:///fixture/flux', 'instance': 'fixture',
                'job': 'fFixture'},
        'scratch': {'leaf': str(leaf)},
        'progress': {'journal': str(output / 'progress.jsonl'),
                     'no_progress_seconds': no_progress_seconds},
        'output_dir': str(output),
        **extra,
    }


    def launch_context():
        # require_admission (often stubbed here) recreates the leaf the workspace removed.
        leaf.mkdir(mode=0o700, exist_ok=True)
        leaf.chmod(0o700)
        return context

    monkeypatch.setenv('TL3_EXECUTION_BACKEND', 'flux')
    monkeypatch.setattr(runtime, 'launch_context', launch_context)
    return context
