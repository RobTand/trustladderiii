"""Run frozen adapter fixtures inside an admitted PB exact-source checkout."""
import hashlib
import json
from pathlib import Path
import sys

root = Path(__file__).resolve().parent
manifest = json.loads((root / "SOURCE.json").read_text())
files = {
    "pb_nightly.py": "pb_nightly.py.source",
    "test_pb_nightly.py": "fixtures.py.source",
    "dev_evidence.candidate.py": "dev_evidence.py.source",
    "qualified-derive_pb_sizing.py": "derive_pb_sizing.py.source",
}
source = {}
for original, packaged in files.items():
    raw = (root / packaged).read_bytes()
    expected = manifest["files"][original]
    if {"sha256": hashlib.sha256(raw).hexdigest(), "bytes": len(raw)} != expected:
        raise RuntimeError("Frozen source identity differs: " + original)
    source[original] = raw.decode()
if len(sys.argv) != 2:
    raise SystemExit("usage: fixture-check.py SAFE_READER_SOURCE")
reader_source = sys.argv[1]
# The safe-reader bytes are sealed in the admitted command, as in producer GREEN.
sys.argv = [sys.argv[0], source["pb_nightly.py"], source["test_pb_nightly.py"],
            reader_source, source["dev_evidence.candidate.py"],
            source["qualified-derive_pb_sizing.py"]]
exec(compile((root / "fixture-program.py.source").read_bytes(), "fixture-program.py", "exec"))
