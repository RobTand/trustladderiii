"""Refresh H18 copies, preserving the immutable reference from the assigned base."""
from pathlib import Path
import re
import subprocess

ROOT = Path(__file__).resolve().parents[4]
HERE = Path(__file__).resolve().parent
BASE = "f5b0ba2fb1"
SOURCE = "crates/controller/src/memory.rs"
CHANGED = ["queue_attempt", "flush", "curate"]
HELPERS = ["clip", "sensitive_key", "redact_text", "safe_text", "sanitize", "section",
           "bound_section", "activity_section", "read", "clip_chars", "uuid", "client",
           "bank_url", "memory_id", "valid_id", "queue_lesson", "normalized", "verify",
           "tokens", "identifier_like", "identifiers", "named_files", "contradicts", "cite",
           "review_memories", "stored_json_bytes", "memory_attempt"]


def function(source, name):
    start = re.search(rf"(?m)^(?:pub(?:\([^)]*\))? )?fn {name}(?:\(|<)", source).start()
    end = source.index("\n}\n", start) + 3
    return source[start:end]


def main():
    old = subprocess.check_output(["git", "show", f"{BASE}:{SOURCE}"], cwd=ROOT).decode()
    frozen = HERE / "base-memory.rs.txt"
    if frozen.exists() and frozen.read_text() != old:
        raise SystemExit("frozen memory source changed")
    frozen.write_text(old)
    current = (ROOT / SOURCE).read_text()
    for name, source in [("reference", old), ("current", current)]:
        (HERE / f"{name}.rs").write_text("use super::*;\n" + "".join(function(source, n) for n in CHANGED))
    assert all(function(old, n) == function(current, n) for n in HELPERS)
    (HERE / "helpers.rs").write_text("".join(function(old, n) for n in HELPERS))
    # Constants/types needed by the unchanged evidence helpers are copied verbatim.
    fragments = [old[old.index("const SECTION_BYTES"):old.index("fn clip(")],
                 old[old.index("const PER_RUN"):old.index("/// What one worker run")],
                 old[old.index("const FILE_EXTENSIONS"):old.index("/// Code-shaped tokens")],
                 old[old.index("struct Run<'a>"):old.index("/// Record a run's memory flags")],
                 old[old.index("#[derive(Clone, Copy, Debug, PartialEq, Eq)]\npub enum LessonKind"):old.index("/// Queue a lesson")]]
    (HERE / "types.rs").write_text("".join(fragments))
    (HERE / "names.rs").write_text('const HELPERS: &[&str] = &'+repr(HELPERS).replace("'", '"')+';\n')


if __name__ == "__main__":
    main()
