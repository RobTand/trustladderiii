import os
import subprocess

from tl3_harness.trees import git


def test_git_ignores_hostile_environment(tmp_path, monkeypatch):
    def fixture(path, text):
        path.mkdir()
        env = {"PATH": os.defpath, "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": os.devnull}
        def run(*args):
            return subprocess.check_output(["git", "-C", str(path), "-c", "user.name=Fixture",
                "-c", "user.email=fixture@example.invalid", "-c", "core.hooksPath=/dev/null", *args], env=env)
        run("init", "-q", "--template=")
        (path / "value").write_text(text)
        run("add", ".")
        run("commit", "-qm", "fixture")
        return run("rev-parse", "HEAD")
    intended, foreign = tmp_path / "intended", tmp_path / "foreign"
    head = fixture(intended, "intended")
    fixture(foreign, "foreign")
    for key, value in {"GIT_DIR": str(foreign / ".git"), "GIT_WORK_TREE": str(foreign),
            "GIT_INDEX_FILE": str(tmp_path / "hostile-index"), "GIT_CONFIG_COUNT": "2",
            "GIT_CONFIG_KEY_0": "protocol.allow", "GIT_CONFIG_VALUE_0": "always",
            "GIT_CONFIG_KEY_1": "credential.helper", "GIT_CONFIG_VALUE_1": "!exit 99"}.items():
        monkeypatch.setenv(key, value)
    assert git(intended, "rev-parse", "HEAD") == head
    for key, expected in {"protocol.allow": b"never", "credential.helper": b"",
            "core.hooksPath": b"/dev/null"}.items():
        assert git(intended, "config", "--get", key).strip() == expected
