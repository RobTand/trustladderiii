"""Flux path spelling must not hide a containment refusal."""
from pathlib import Path
import tempfile

import pytest

from tl3_harness import execution, runtime
from tl3_harness.runtime_evidence import TargetEnvironmentError


@pytest.fixture
def paths():
    with tempfile.TemporaryDirectory(prefix='tl3-paths-', dir='/tmp') as directory:
        root = Path(directory)
        for name in ('checkout', 'scratch', 'harness', 'flux-vanilla/ƒhm2UMeMcT'):
            (root / name).mkdir(parents=True, mode=0o700)
        yield root


@pytest.mark.parametrize('mount', ['checkout', 'scratch', 'harness'])
def test_unicode_flux_mount_is_a_typed_refusal(paths, mount):
    mounts = {name: paths / name for name in ('checkout', 'scratch', 'harness')}
    mounts[mount] = paths / 'flux-vanilla/ƒhm2UMeMcT' / mount
    mounts[mount].mkdir()
    assert all(path.is_dir() for path in mounts.values())
    with pytest.raises(TargetEnvironmentError) as caught:
        runtime.podman_command(mounts['checkout'], 'sha256:' + 'b' * 64,
                              mounts['scratch'], 1, mode='in_job', harness=mounts['harness'])
    assert caught.value.reason == 'invalid_mount_path'


def test_run_tree_preserves_typed_prelaunch_refusal(paths, monkeypatch):
    tree = paths / 'flux-vanilla/ƒhm2UMeMcT/checkout'
    tree.mkdir()
    (tree / 'test_value.py').write_text('def test_value():\n    assert True\n')
    monkeypatch.setattr(runtime, 'journal_path', lambda path: path)
    monkeypatch.setattr(runtime, 'temporary_root', lambda: paths / 'scratch')
    monkeypatch.setattr(execution, 'scratch_directory', lambda: paths / 'scratch')

    def command(tree, scratch):
        return runtime.podman_command(tree, 'sha256:' + 'b' * 64, scratch, 1,
                                      mode='in_job', harness=paths / 'harness')

    monkeypatch.setattr(runtime, 'command', command)
    with pytest.raises(TargetEnvironmentError) as caught:
        execution.run_tree(tree, execution.Container('sha256:' + 'b' * 64),
                           ('test_value.py::test_value',), paths / 'report.jsonl', None)
    assert caught.value.reason == 'invalid_mount_path'
    assert (tree / 'test_value.py').is_file()
    assert (paths / 'report.jsonl').is_file()


def test_workspace_uses_canonical_context_under_unicode_tmpdir(paths, monkeypatch):
    inherited = paths / 'flux-vanilla/ƒhm2UMeMcT'
    monkeypatch.setenv('TMPDIR', str(inherited))
    monkeypatch.setenv('FLUX_JOB_TMPDIR', str(inherited))
    monkeypatch.setattr(tempfile, 'tempdir', str(inherited))
    leaf = paths / 'scratch/fABC'
    leaf.mkdir(mode=0o700)
    context = {'scratch': {'leaf': str(leaf)}}
    monkeypatch.setattr(runtime, 'launch_context', lambda: context)
    monkeypatch.setattr(runtime, '_uncertain_containers', set())
    records = []
    monkeypatch.setattr(runtime, '_action_record', lambda *args: records.append(args))
    with runtime.workspace(prefix='tl3-checks-') as directory:
        tree = directory / 'base'
        tree.mkdir()
        command = runtime.podman_command(tree, 'sha256:' + 'b' * 64, directory, 1,
                                         mode='in_job', harness=paths / 'harness')
        assert f'type=bind,src={tree},dst=/checkout,readonly' in command
        assert f'type=bind,src={directory},dst=/tmp' in command
        assert '--cgroups=disabled' in command
        assert str(directory).isascii()
    assert records[0][1] == 'action-scratch-absence.json'
    assert inherited.is_dir()
