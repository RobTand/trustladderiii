"""Regression for #935: proof receipts must not depend on the host umask."""
import os

import pytest

from tl3_harness.model import Check, Kind
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
