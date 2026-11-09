"""Generator preparation: read-only checkout staging and bounded failure causes.

Run inside admitted development compute as an unprivileged user. A root caller
bypasses directory permissions and would hide the 0500 staging defect.
"""
import json
import os
from pathlib import Path
import stat
import subprocess

import pytest

from tl3_harness import generator


IMAGE = "sha256:" + "a" * 64
SECRET = "ghp_" + "Z" * 36


def contract(**overrides):
    value = {"command": "python3 tools/gen.py", "generated_files": ["generated/*"],
             "protected_paths": [], "files": [], "timeout": 30, "cpus": 1, "mem_gb": 1}
    value.update(overrides)
    return value


def readonly_tree(root):
    tree = root / "checkout"
    (tree / "tools").mkdir(parents=True)
    (tree / "tools/gen.py").write_text("print('generate')\n")
    (tree / "data.txt").write_text("immutable input\n")
    (tree / "tools/gen.py").chmod(0o555)
    (tree / "data.txt").chmod(0o444)
    (tree / "tools").chmod(0o500)
    tree.chmod(0o500)
    return tree


def modes(tree):
    return {str(path.relative_to(tree)): stat.S_IMODE(path.lstat().st_mode)
            for path in [tree, *sorted(tree.rglob("*"))]}


def release(tree):
    for path in [tree, *tree.rglob("*")]:
        if path.is_dir():
            path.chmod(0o700)


@pytest.fixture(autouse=True)
def unprivileged():
    assert os.geteuid() != 0, "root bypasses 0500 permissions; run as the unprivileged fleet user"


@pytest.fixture
def admitted(monkeypatch):
    # The admission gate checks the PB/Flux environment, not generator staging.
    monkeypatch.setattr(generator, "require_admission", lambda: None)


def test_readonly_checkout_stages_writable_copy_before_git_and_reaches_container(tmp_path, admitted, monkeypatch):
    tree = readonly_tree(tmp_path)
    before = modes(tree)
    staged = {}

    def container_command(copy, container, argv, cidfile):
        info = copy.stat()
        listed = subprocess.run(["git", "-C", str(copy), "ls-files", "-s"], check=True,
                                stdout=subprocess.PIPE, text=True).stdout
        staged.update(root=copy.parent, writable=bool(info.st_mode & stat.S_IWUSR),
                      git=(copy / ".git").is_dir(), listed=listed,
                      executable=bool((copy / "tools/gen.py").stat().st_mode & stat.S_IXUSR),
                      git_mode=stat.S_IMODE((copy / ".git").stat().st_mode))
        return ["/bin/true"]

    monkeypatch.setattr(generator, "container_command", container_command)
    try:
        result = generator.run(tree, generator.Container(IMAGE), **contract())
        assert result["status"] == "completed", result
        assert result["timings"]["preparation"]["outcome"] == "completed", result
        assert result["timings"]["container"]["outcome"] == "completed", result
        assert result["timings"]["validation"]["outcome"] == "completed", result
        assert result["outputs"] == [] and "cause" not in result, result
        assert staged["writable"] and staged["git"] and staged["executable"], staged
        assert "100755 " in staged["listed"] and "tools/gen.py" in staged["listed"], staged
        assert "data.txt" in staged["listed"], staged
        # Synthetic Git metadata keeps Git's own umask-derived mode; the generator
        # opens only the checkout copy and never widens .git to 0777.
        umask = os.umask(0)
        os.umask(umask)
        assert staged["git_mode"] == 0o777 & ~umask, (oct(staged["git_mode"]), oct(umask))
        assert not staged["root"].exists(), "generator scratch copy was not removed"
        assert modes(tree) == before, "immutable source tree changed"
    finally:
        release(tree)


def test_preparation_failure_reports_bounded_safe_cause_without_secret(tmp_path, admitted, monkeypatch, capsys):
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


def test_git_failure_cause_never_carries_multiline_stderr_or_argv(tmp_path, admitted, monkeypatch):
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


def test_flux_generator_refuses_unowned_container_launch_before_any_runtime_call(tmp_path, admitted, monkeypatch):
    # Flux containers must go through the durable launch/binding protocol. Until the
    # generator has that path it must refuse in preparation and never start Podman.
    tree = tmp_path / "checkout"
    tree.mkdir()
    (tree / "data.txt").write_text("input\n")
    monkeypatch.setattr("tl3_harness.runtime.launch_context", lambda: {"fixture": True})
    # Allocation and scratch release are exercised by the Rust collector e2e.
    monkeypatch.setattr(generator, "_flux_allocation", lambda: None, raising=False)
    monkeypatch.setattr(generator, "_release_flux_scratch", lambda: None, raising=False)
    real = subprocess.run
    clients = []

    def guarded(argv, *args, **kwargs):
        if Path(str(argv[0])).name in ("podman", "docker"):
            clients.append(argv)
            raise AssertionError("no container runtime client may run")
        return real(argv, *args, **kwargs)

    monkeypatch.setattr(subprocess, "run", guarded)
    try:
        result = generator.run(tree, generator.Container(IMAGE), **contract())
    except Exception as error:
        pytest.fail(f"generator escaped its typed failure frame: {type(error).__name__}")
    assert not clients, clients
    assert result["status"] == "failed" and result["phase"] == "preparation", result
    assert result["timings"]["container"]["outcome"] == "skipped", result
    assert result["cause"] == {"operation": "container_command", "type": "FluxContainerUnsupported",
                               "errno": None, "detail": None}, result
