"""Refresh compiled production copies; never regenerate the frozen reference."""
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parent
REPO = ROOT.parents[3]
BASE = "cdacce8f99c28ca8dba8f5f76831b77b4dfcd836"


def function(source, name, production=False):
    after = source.index("impl Executor for Adapter {") if production and name in {"execute", "reconcile"} else 0
    start = source.index(f"    fn {name}(", after)
    end = source.index("\n    }\n", start) + 7
    return source[start:end]


def refresh(template, source, production):
    for name in ("read", "heal_body", "phase", "completion", "execute", "reconcile", "read_enqueue", "baseline", "dequeue_pending", "snapshot", "fence"):
        template = template.replace(function(template, name), function(source, name, production))
    # Boundary projection belongs to the compiled shell as well.
    marker = "fn receipt_read("
    if marker in source:
        start = source.index(marker)
        end = source.index("\n}\n", start) + 3
        if marker in template:
            old_start = template.index(marker)
            old_end = template.index("\n}\n", old_start) + 3
            template = template[:old_start] + template[old_end:]
        template += "\n" + source[start:end]
    return template


def main():
    source = (REPO / "crates/controller/src/adapters/mergify.rs").read_text()
    template = (REPO / "crates/controller/tests/mergify_io/current.rs").read_text()
    (ROOT / "current.rs").write_text(refresh(template, source, True))
    reference = ROOT / "reference.rs"
    if not reference.exists():
        frozen = subprocess.check_output(["git", "show", f"{BASE}:crates/controller/src/adapters/mergify.rs"], cwd=REPO).decode()
        (ROOT / "base-adapter.rs.txt").write_text(frozen)
        reference.write_text(refresh(template, frozen, True))
        core = subprocess.check_output(["git", "show", f"{BASE}:crates/mergify/src/decide/enqueue.rs"], cwd=REPO).decode()
        (ROOT / "base-enqueue.rs.txt").write_text(core)


if __name__ == "__main__":
    main()
