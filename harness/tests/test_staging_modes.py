"""Regression for #935: proof receipts must not depend on the host umask."""
import base64
import json
import os
from pathlib import Path

import pytest

from tl3_harness.model import Check, Forcing, Kind, Proof
from tl3_harness.proof import assemble_proof
from tl3_harness.trees import tree_sha256


def test_assembly_receipt_is_independent_of_umask(tmp_path):
    base, head = tmp_path / 'base', tmp_path / 'head'
    base.mkdir()
    head.mkdir()
    observer = head / 'tests' / 'nested' / 'test_observer.py'
    observer.parent.mkdir(parents=True)
    observer.write_text('def test_observer():\n    assert False\n')
    observer.chmod(0o644)
    check = Check('observer', Kind.DEMONSTRATING,
                  ('tests/nested/test_observer.py::test_observer',))
    receipts, digests, modes = [], [], []
    # This target runs in one process without threads; always restore the umask.
    for mask in (0o022, 0o002, 0o077):
        destination = tmp_path / f'staged-{mask}'
        previous = os.umask(mask)
        try:
            receipts.append(assemble_proof(base, head, destination, check,
                                           {'test-files': ['tests/**/*.py']}))
        finally:
            os.umask(previous)
        digests.append(tree_sha256(destination))
        modes.append({path.relative_to(destination).as_posix(): path.stat().st_mode & 0o777
                      for path in destination.rglob('*')})
    assert receipts[0] == receipts[1] == receipts[2]
    assert digests[0] == digests[1] == digests[2]
    assert modes == [{'tests': 0o755, 'tests/nested': 0o755,
                      'tests/nested/test_observer.py': 0o644}] * 3


@pytest.mark.parametrize('case', json.loads(
    (Path(__file__).parent / 'fixtures/staging_modes/replay.json').read_text()),
    ids=lambda case: f"corpus-{case['corpus_id']}")
def test_spike_replay_modes(tmp_path, case):
    """#935: actual #902 inputs retain the original 022 receipt under every umask."""
    data = case['input']
    for side in ('base', 'head'):
        (tmp_path / side).mkdir()
        for name, value in data[side].items():
            path = tmp_path / side / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(base64.b64decode(value['b64']))
            path.chmod(value['mode'])
    declaration = data['check']
    check = Check(**{**declaration, 'kind': Kind(declaration['kind']),
                     'proof': Proof.parse(declaration['proof']) if declaration['proof'] else None,
                     'forcing': Forcing.parse(declaration['forcing']) if declaration['forcing'] else None})
    receipts = []
    for mask in (0o022, 0o002, 0o077):
        previous = os.umask(mask)
        try:
            destination = tmp_path / f'staged-{mask}'
            receipts.append(assemble_proof(tmp_path / 'base', tmp_path / 'head', destination,
                                           check, data['config']))
        finally:
            os.umask(previous)
        assert tree_sha256(destination) == case['staged_tree_sha256']
        for path in destination.rglob('*'):
            name = path.relative_to(destination).as_posix()
            if path.is_file() and name not in data['base']:
                assert path.stat().st_mode & 0o777 == 0o644
    assert receipts[0] == receipts[1] == receipts[2]


@pytest.mark.parametrize('base_mode,head_mode', [(0o755, 0o644), (0o644, 0o755)])
def test_grafts_keep_base_modes_and_new_modules_use_fixed_modes(tmp_path, base_mode, head_mode):
    base, head = tmp_path / 'base', tmp_path / 'head'
    for tree, mode in ((base, base_mode), (head, head_mode)):
        (tree / 'tests').mkdir(parents=True)
        observer = tree / 'tests/test_existing.py'
        observer.write_text(f'def test_existing():\n    assert {tree == head}\n')
        observer.chmod(mode)
        (tree / 'tests').chmod(0o750)
        script = tree / 'script.py'
        script.write_text('print("preserved")\n')
        script.chmod(0o755)
    new = head / 'tests/nested/test_new.py'
    new.parent.mkdir()
    new.write_text('def test_new():\n    assert False\n')
    new.chmod(0o755)
    check = Check('observers', Kind.DEMONSTRATING,
                  ('tests/test_existing.py::test_existing', 'tests/nested/test_new.py::test_new'))
    receipts = []
    for mask in (0o022, 0o002, 0o077):
        destination = tmp_path / f'staged-{mask}'
        previous = os.umask(mask)
        try:
            receipts.append(assemble_proof(base, head, destination, check,
                                           {'test-files': ['tests/**/*.py']}))
        finally:
            os.umask(previous)
        assert (destination / 'script.py').stat().st_mode & 0o777 == 0o755
        assert (destination / 'tests/test_existing.py').stat().st_mode & 0o777 == base_mode
        assert (destination / 'tests').stat().st_mode & 0o777 == 0o750
        assert (destination / 'tests/nested').stat().st_mode & 0o777 == 0o755
        assert (destination / 'tests/nested/test_new.py').stat().st_mode & 0o777 == 0o644
        assert (head / 'tests/test_existing.py').stat().st_mode & 0o777 == head_mode
        assert new.stat().st_mode & 0o777 == 0o755
    assert receipts[0] == receipts[1] == receipts[2]
