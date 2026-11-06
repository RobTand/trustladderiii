"""Regression for #935: proof receipts must not depend on the host umask."""
import os

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
    for mask in (0o022, 0o002):
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
    assert receipts[0] == receipts[1]
    assert digests[0] == digests[1]
    assert modes == [{'tests': 0o755, 'tests/nested': 0o755,
                      'tests/nested/test_observer.py': 0o644}] * 2
