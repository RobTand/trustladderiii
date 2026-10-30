"""The run_tests factory. Only selectors are exposed to the model."""
from dataclasses import dataclass
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import subprocess
import sys
import tempfile


@dataclass(frozen=True)
class TestSettings:
    image: str
    tools_path: Path
    cpus: int = 1
    mem_gb: int = 2
    quiet_seconds: int = 120

    def __post_init__(self):
        if not re.fullmatch(r"(?:[\w./:-]+@)?sha256:[0-9a-f]{64}", self.image):
            raise ValueError("Use an immutable container image digest")
        if not Path(self.tools_path).is_absolute():
            raise ValueError("PrismaBuild tools_path must be absolute")
        if any(type(v) is not int or v < 1 for v in (self.cpus, self.mem_gb, self.quiet_seconds)):
            raise ValueError("Test resource and quiet allowances must be positive integers")


# Avoid pytest treating this settings type as a test class.
TestSettings.__test__ = False


def _selectors(tests):
    if not isinstance(tests, list) or len(tests) > 256:
        raise ValueError("Use a list of at most 256 test selectors")
    for test in tests:
        if (not isinstance(test, str) or not test or len(test) > 2048
                or any(c in test for c in ("\x00", "\\", "\n", "\r"))):
            raise ValueError("Use relative pytest paths or node IDs")
        path = test.split("::", 1)[0]
        if (not path or path.startswith(("/", "-"))
                or any(part in ("..", ".git") for part in PurePosixPath(path).parts)):
            raise ValueError("Use relative pytest paths or node IDs")
    return tuple(dict.fromkeys(tests))


def _stage(worktree, root):
    from tl3_harness import execution

    checkout = root / "checkout"
    checkout.mkdir()
    with worktree._directory(()) as directory:
        for name, is_dir in worktree._walk(directory):
            target = checkout / name
            if is_dir:
                target.mkdir()
            else:
                with worktree._file(name) as source, target.open("wb") as destination:
                    shutil.copyfileobj(source.buffer, destination)
    trusted = root / "trusted"
    trusted.mkdir()
    shutil.copytree(Path(execution.__file__).parent, trusted / "tl3_harness",
                    ignore=shutil.ignore_patterns("__pycache__", "*.pyc"))
    (root / "action.py").write_text(
        'import sys\nfrom pathlib import Path\n'
        'sys.path.insert(0, str(Path(__file__).resolve().parent / "trusted"))\n'
        'from tl3_harness.test_tool import main\nmain()\n'
    )
    # PB snapshots a Git checkout. These fixed commands create its private input,
    # never invoking hooks or commands from the role's Git configuration.
    env = {**os.environ, "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": os.devnull,
           "GIT_AUTHOR_NAME": "TL3", "GIT_AUTHOR_EMAIL": "tl3@example.invalid",
           "GIT_COMMITTER_NAME": "TL3", "GIT_COMMITTER_EMAIL": "tl3@example.invalid"}
    for args in (("init", "--quiet", "--template="), ("add", "--force", "--all"),
                 ("-c", "core.hooksPath=/dev/null", "-c", "commit.gpgsign=false",
                  "commit", "--quiet", "-m", "Freeze test input")):
        subprocess.run(["git", *args], cwd=root, env=env, check=True,
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def _json_command(command):
    with tempfile.TemporaryFile() as output:
        result = subprocess.run(command, stdout=output, stderr=subprocess.DEVNULL)
        output.seek(0)
        raw = output.read(2_097_153)
    if len(raw) > 2_097_152:
        raise ValueError("PrismaBuild response exceeded its limit")
    return result.returncode, json.loads(raw)


def _summary(ending, key):
    from tl3_harness.test_tool import PREFIX, SUMMARY_BYTES

    if (ending["action_key"] != key or ending["status"] not in ("executed", "cache_hit")
            or ending["exit_code"] != 0 or not ending["succeeded"]):
        raise ValueError("PrismaBuild action did not succeed")
    evidence = ending["result"]
    if evidence["verified"] is not True or not evidence["receipt_sha256"]:
        raise ValueError("PrismaBuild result lacks verified receipt evidence")
    log = evidence["stdout"]
    if type(log["bytes"]) is not int or not 0 <= log["bytes"] <= 1_048_576:
        raise ValueError("PrismaBuild result log exceeded its limit")
    with open(log["path"], "rb") as stream:
        raw = stream.read(1_048_577)
    if len(raw) != log["bytes"] or hashlib.sha256(raw).hexdigest() != log["sha256"]:
        raise ValueError("PrismaBuild result log changed")
    records = [line[len(PREFIX):] for line in raw.decode().splitlines() if line.startswith(PREFIX)]
    if len(records) != 1:
        raise ValueError("PrismaBuild returned no unique test summary")
    result = json.loads(records[0])
    result["action_key"] = key
    if (result["status"] not in ("completed", "infrastructure_error")
            or len(json.dumps(result).encode()) > SUMMARY_BYTES):
        raise ValueError("PrismaBuild returned an invalid test summary")
    return result


def make_test_tool(worktree, settings: TestSettings | None = None):
    """Bind settings from role inputs, directly or via worktree.test_settings."""
    settings = settings if settings is not None else getattr(worktree, "test_settings", None)

    def run_tests(tests: list[str] | None = None) -> dict:
        """Run relative pytest paths/node IDs, or the whole suite when omitted."""
        from tl3_harness.test_tool import infrastructure

        selected = _selectors([] if tests is None else tests)
        key = None
        try:
            if not isinstance(settings, TestSettings):
                return infrastructure("PrismaBuild test settings were not supplied by the role.")
            with tempfile.TemporaryDirectory(prefix="tl3-test-input-") as temporary:
                root = Path(temporary)
                _stage(worktree, root)
                command = [sys.executable, str(Path(settings.tools_path) / "pbrun.py"),
                           "--cwd", str(root), "--anywhere", "--cpus", str(settings.cpus),
                           "--demand", f"mem_gb={settings.mem_gb}",
                           "--container-image", settings.image,
                           "--progress-phase", f"tests={settings.quiet_seconds}",
                           "--env", "OMP_NUM_THREADS=1", "--env", "MKL_NUM_THREADS=1",
                           "--env", "OPENBLAS_NUM_THREADS=1", "--detach", "--",
                           "python3", "-I", "-B", "action.py", settings.image, json.dumps(selected)]
                code, submitted = _json_command(command)
                if code or submitted.get("schema") != "prismabuild.submission.v1":
                    raise ValueError("PrismaBuild refused submission")
                key = submitted["action_key"]
                if not isinstance(key, str) or not re.fullmatch("[0-9a-f]{64}", key):
                    key = None
                    raise ValueError("PrismaBuild returned an invalid action key")
                # pbwait's observation allowance is not an execution deadline.
                # Reattach to the same action whenever that allowance expires.
                while True:
                    code, waited = _json_command([
                        sys.executable, str(Path(settings.tools_path) / "pbwait.py"),
                        "--endings", "--until", "cleaned", key,
                    ])
                    if waited.get("schema") != "prismabuild.endings.v1":
                        raise ValueError("PrismaBuild returned an invalid ending")
                    endings = waited["endings"]
                    if len(endings) != 1 or endings[0]["action_key"] != key:
                        raise ValueError("PrismaBuild returned another action")
                    if waited["complete"]:
                        if code:
                            raise ValueError("PrismaBuild action infrastructure failed")
                        return _summary(endings[0], key)
                    if endings[0]["status"] not in ("waiting", "executed", "cache_hit"):
                        raise ValueError("PrismaBuild could not verify completion")
        except (OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError):
            result = infrastructure("PrismaBuild submission, execution or verified result retrieval failed.")
            if key:
                result["action_key"] = key
            return result

    return run_tests
