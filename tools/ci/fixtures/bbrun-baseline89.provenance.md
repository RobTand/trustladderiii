# Frozen baseline89 cleanup control

Found by: actual remote gate. Owner: ops, TL3 #878.

`bbrun-baseline89.py.txt` contains the exact historical `tools/ci/bbrun.py`
bytes from commit `6c6efd421c7026b9277c6af3ebb27c74030219fe`.

- Git blob: `ecf6e3841c395008f60996801aafc8223bbe5d75`.
- SHA256: `89c8a850b9381cd2e4d52a2d94f45b5c2272c9fcb7a3708ef2a8c45552836763`.
- Production source path: `tools/ci/bbrun.py`.

The `.txt` file is test data, not an importable module or a production client.
`test_baseline89_red` reads it from Bazel runfiles, checks the SHA256 before
loading a temporary Python copy, and executes the real baseline behavior.
It must still observe `PermissionError`, nonzero exit, residual unscanned
upload bytes and no wrapper receipt on the nested readonly-tree fixture.
Do not update these bytes to the fixed wrapper or weaken those assertions.
Missing or changed fixture bytes must fail the test.

The `tl3_py_test` macro declares this package's runtime files with
`native.glob(["**"])`, which includes this fixture. No Git executable,
repository metadata, historical commit object, network fetch or fallback is
needed inside the sandbox. Gazelle target registration remains captain-owned.

Retained sandbox RED is parent
`a52e724a-aa37-4d00-924f-f0a0182d5f4c`, child
`863ddccb-1ddd-4e7a-8c23-4148e1ccbe3e`, at FX head
`b49f9219308c42115a903ed09dc04c3dc9742ee8`.
The control failed to set up because `git show` ran without a Git repository;
the other 17 cleanup cases passed. This fixture fixes that input dependency
without exempting, skipping or removing the failing-baseline control.
