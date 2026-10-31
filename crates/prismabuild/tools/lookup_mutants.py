"""Prove the finish regression rejects scanning captures just before withdrawn.

Run inside an admitted PrismaBuild action from a clean scratch checkout.
The original source is restored even when the regression unexpectedly passes.
"""
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[3]


def main():
    if subprocess.check_output(
        ["git", "status", "--porcelain", "--untracked-files=no"], cwd=ROOT
    ).strip():
        raise SystemExit("mutants require a clean scratch checkout")
    path = ROOT / "crates/prismabuild/src/queue.rs"
    source = path.read_text()
    old = "if index == 2 && chosen.is_none()"
    assert source.count(old) == 1, "expected exactly one capture scan"
    path.write_text(source.replace(old, "if index == 4 && chosen.is_none()"))
    try:
        result = subprocess.run(
            [
                "cargo", "test", "-p", "tl3-prismabuild", "--lib", "--locked",
                "lookup_keeps_finish_present_when_marker_retires_after_terminals",
                "--", "--nocapture",
            ],
            cwd=ROOT, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
        )
        print(result.stdout, flush=True)
        expected = "finish after done: expected moving or successful terminal, got Ok(Absent)"
        assert result.returncode == 101 and expected in result.stdout, (
            f"scan-position mutant did not fail at {expected!r}, exit {result.returncode}"
        )
        print("RED VERIFIED mutant=lookup-scan-position exit=101 assertion=finish after done", flush=True)
    finally:
        path.write_text(source)


if __name__ == "__main__":
    main()
