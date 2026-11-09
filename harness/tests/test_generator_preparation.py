"""Generator preparation: bounded failure causes inside an admitted Flux job.

Run as an unprivileged user. Read-only checkout staging runs end to end on the
owned launch path in test_fx_generator_exec.py.
"""
import json
import os
import subprocess

import pytest

from tl3_harness import generator, runtime


IMAGE = "sha256:" + "a" * 64
SECRET = "ghp_" + "Z" * 36


def contract(**overrides):
    value = {"command": "python3 tools/gen.py", "generated_files": ["generated/*"],
             "protected_paths": [], "files": [], "timeout": 30, "cpus": 1, "mem_gb": 1}
    value.update(overrides)
    return value


@pytest.fixture(autouse=True)
def unprivileged():
    assert os.geteuid() != 0, "root bypasses permissions; run as the unprivileged fleet user"


@pytest.fixture
def flux(tmp_path, monkeypatch):
    """The Flux job context. Admission and scratch are real; the allocation grant is
    exercised by test_fx_generator_exec.py and the Rust collector e2e."""
    xdg = tmp_path / "xdg"
    xdg.mkdir(mode=0o700)
    monkeypatch.setenv("HOME", str(tmp_path))
    monkeypatch.setenv("XDG_RUNTIME_DIR", str(xdg))
    output = tmp_path / "output"
    output.mkdir()
    scratch = tmp_path / "scratch"
    scratch.mkdir(mode=0o700)
    context = {"output_dir": str(output), "scratch": {"approved_root": str(scratch), "leaf": str(scratch / "fJob")}}
    monkeypatch.setattr(runtime, "launch_context", lambda: context)
    monkeypatch.setattr(generator, "_flux_allocation", lambda: None)
    return {"leaf": scratch / "fJob", "output": output}


def test_preparation_failure_reports_bounded_safe_cause_without_secret(tmp_path, flux, monkeypatch, capsys):
    tree = tmp_path / "checkout"
    tree.mkdir()
    blocked = tree / f"token-{SECRET}.txt"
    blocked.write_text("unreadable\n")
    blocked.chmod(0)
    (tmp_path / "generator.json").write_text(json.dumps(contract()))
    monkeypatch.chdir(tmp_path)
    monkeypatch.setattr("sys.argv", ["generator", IMAGE])
    launched = []
    monkeypatch.setattr(generator, "container_command", lambda *args: launched.append(args) or ["/bin/false"])
    try:
        generator.main()
    finally:
        blocked.chmod(0o600)
    lines = capsys.readouterr().out.splitlines()
    assert len(lines) == 1 and lines[0].startswith(generator.PREFIX), lines
    frame = lines[0]
    result = json.loads(frame[len(generator.PREFIX):])
    assert SECRET not in frame and str(tmp_path) not in frame, frame
    assert result["status"] == "failed" and result["exit_code"] is None, result
    assert result["phase"] == "preparation", result
    assert result["timings"]["preparation"]["outcome"] == "failed", result
    assert result["timings"]["container"]["outcome"] == "skipped", result
    assert result["timings"]["validation"]["outcome"] == "skipped", result
    assert "outputs" not in result and "output_manifest" not in result, result
    assert result["cause"] == {"operation": "input_manifest", "type": "PermissionError",
                               "errno": 13, "detail": "Permission denied"}, result
    assert not launched
    # No container began, so the owned job scratch is released with absence evidence.
    assert not flux["leaf"].exists()
    assert (flux["output"] / "action-scratch-absence.json").is_file()


def test_git_failure_cause_never_carries_multiline_stderr_or_argv(tmp_path, flux, monkeypatch):
    tree = tmp_path / "checkout"
    tree.mkdir()
    (tree / "data.txt").write_text("input\n")
    noisy = (f"fatal: https://fixture:{SECRET}@example.invalid/repo\nsecond line\n" * 512).encode()

    def failing(repo, *args):
        raise subprocess.CalledProcessError(128, ["git", "-c", f"http.extraHeader=Bearer {SECRET}", *args],
                                            output=noisy, stderr=noisy)

    monkeypatch.setattr(generator, "git", failing)
    result = generator.run(tree, generator.Container(IMAGE), **contract())
    text = json.dumps(result)
    assert SECRET not in text and "second line" not in text, text[:2000]
    assert len(text) < 4096, len(text)
    assert result["status"] == "failed" and result["phase"] == "preparation", result
    assert result["timings"]["container"]["outcome"] == "skipped", result
    assert result["cause"] == {"operation": "git_init", "type": "CalledProcessError",
                               "errno": None, "detail": "exit status 128"}, result
    assert not flux["leaf"].exists()


def test_generator_identity_is_the_contract_implementation():
    # crates/contracts maker_reply.rs pins generation evidence to this implementation.
    assert generator.IMPLEMENTATION == "tl3-generator-v4"
