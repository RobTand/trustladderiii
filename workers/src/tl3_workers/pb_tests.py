"""The run_tests factory. The role supplies the base and execution settings."""
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

from tl3_harness import execution
from tl3_harness.test_tool import PREFIX, SUMMARY_BYTES, infrastructure
from tl3_harness.trees import export


@dataclass(frozen=True)
class TestSettings:
    image: str
    tools_path: Path
    cpus: int = 1
    mem_gb: int = 2
    quiet_seconds: int = 120
    exec_scratch: bool = False

    def __post_init__(self):
        if type(self.exec_scratch) is not bool:
            raise ValueError("exec_scratch must be a boolean")
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


def _stage(worktree, root, base_sha=None):
    harness = Path(execution.__file__).resolve().parent
    owner = os.fstat(worktree._root)
    if any(os.path.samestat(parent.stat(), owner) for parent in (harness, *harness.parents)):
        raise ValueError("Install the trusted harness outside the role worktree")
    checkout = root / "checkout"
    if base_sha is not None:
        export(worktree.test_checkout, base_sha, checkout)
    else:
        checkout.mkdir()
        with worktree._directory(()) as directory:
            for name, is_dir in worktree._walk(directory):
                target = checkout / name
                if is_dir:
                    target.mkdir()
                else:
                    with worktree._file(name) as source, target.open("wb") as destination:
                        shutil.copyfileobj(source.buffer, destination)
                        os.fchmod(destination.fileno(), os.fstat(source.fileno()).st_mode & 0o777)
    trusted = root / "trusted"
    trusted.mkdir()
    shutil.copytree(harness, trusted / "tl3_harness",
                    ignore=shutil.ignore_patterns("__pycache__", "*.pyc"))
    (root / "action.py").write_text(
        'import sys\nfrom pathlib import Path\n'
        'sys.path.insert(0, str(Path(__file__).resolve().parent / "trusted"))\n'
        'from tl3_harness.test_tool import main\nmain()\n'
    )
    # PB snapshots a Git checkout. These fixed commands create its private input,
    # never invoking hooks or commands from the role's Git configuration.
    commands = [("init", "--quiet", "--template=")]
    maker_checkout = getattr(worktree, "test_checkout", None)
    if maker_checkout is not None:
        # PB's snapshot parent proves this action belongs to the maker's checkout.
        # Keep the private harness input dirty so PB seals it above that parent.
        commands.extend([
            ("fetch", "--no-tags", "--no-recurse-submodules", "--", str(maker_checkout), "HEAD"),
            ("update-ref", "HEAD", "FETCH_HEAD"),
        ])
    commands.append(("add", "--force", "--all"))
    if maker_checkout is None:
        commands.append(("commit", "--quiet", "-m", "Freeze test input"))
    for args in commands:
        _git(root, *args)


def _git(root, *args):
    env = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
    env.update(GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL=os.devnull,
               GIT_TERMINAL_PROMPT="0", GIT_NO_LAZY_FETCH="1",
               GIT_EDITOR="true", GIT_SEQUENCE_EDITOR="true",
               GIT_AUTHOR_NAME="TL3", GIT_AUTHOR_EMAIL="tl3@example.invalid",
               GIT_COMMITTER_NAME="TL3", GIT_COMMITTER_EMAIL="tl3@example.invalid")
    subprocess.run(["git", "--no-replace-objects", "-c", "core.hooksPath=/dev/null",
                    "-c", "credential.helper=", "-c", "protocol.allow=never",
                    "-c", "protocol.file.allow=always", "-c", "core.fsmonitor=false",
                    "-c", "commit.gpgsign=false", *args],
                   cwd=root, env=env, check=True,
                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def _json_command(command):
    with tempfile.TemporaryFile() as output:
        result = subprocess.run(command, stdout=output, stderr=subprocess.DEVNULL)
        output.seek(0)
        raw = output.read(2_097_153)
    if len(raw) > 2_097_152:
        raise ValueError("PrismaBuild response exceeded its limit")
    value = json.loads(raw)
    if not isinstance(value, dict):
        raise ValueError("PrismaBuild response must be an object")
    return result.returncode, value


def _summary(ending, key):
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


def make_test_tool(worktree, settings: TestSettings | None = None, *, emit=None):
    """Bind settings from role inputs, directly or via worktree.test_settings."""
    settings = settings if settings is not None else getattr(worktree, "test_settings", None)
    emit = emit if emit is not None else getattr(worktree, "progress_emit", lambda event: None)

    def run_tests(tests: list[str] | None = None, base: bool = False) -> dict:
        """Run pytest paths/node IDs. Set base=true to run the role's check base."""
        selected = _selectors([] if tests is None else tests)
        key = None
        try:
            if not isinstance(settings, TestSettings):
                return infrastructure("PrismaBuild test settings were not supplied by the role.")
            base_sha = getattr(worktree, "check_base_sha", None) if base else None
            if base and (not isinstance(base_sha, str)
                         or not re.fullmatch(r"(?:[0-9a-f]{40}|[0-9a-f]{64})", base_sha)
                         or getattr(worktree, "test_checkout", None) is None):
                return infrastructure("The role did not supply a check base and checkout.")
            with tempfile.TemporaryDirectory(prefix="tl3-test-input-") as temporary:
                root = Path(temporary)
                _stage(worktree, root, base_sha)
                command = [sys.executable, str(Path(settings.tools_path) / "pbrun.py"),
                           "--cwd", str(root), "--anywhere", "--cpus", str(settings.cpus),
                           "--demand", f"mem_gb={settings.mem_gb}",
                           "--container-image", settings.image,
                           "--progress-phase", f"tests={settings.quiet_seconds}",
                           "--env", "OMP_NUM_THREADS=1", "--env", "MKL_NUM_THREADS=1",
                           "--env", "OPENBLAS_NUM_THREADS=1", "--detach", "--",
                           "python3", "-I", "-B", "action.py", settings.image, json.dumps(selected)]
                if settings.exec_scratch:
                    command.append("--exec-scratch")
                code, submitted = _json_command(command)
                if code or submitted.get("schema") not in (
                        "prismabuild.submission.v1", "prismaquant.prismabuild.pbrun_detach.v1"):
                    raise ValueError("PrismaBuild refused submission")
                key = submitted["action_key"]
                if not isinstance(key, str) or not re.fullmatch("[0-9a-f]{64}", key):
                    key = None
                    raise ValueError("PrismaBuild returned an invalid action key")
                # pbwait's observation allowance is not an execution deadline.
                # Reattach to the same action whenever that allowance expires.
                while True:
                    emit({"event": "progress", "unit": "external_wait",
                          "wait": {"upstream": "prismabuild", "key": key}})
                    code, waited = _json_command([
                        sys.executable, str(Path(settings.tools_path) / "pbwait.py"),
                        "--endings", "--until", "cleaned", key,
                    ])
                    if waited.get("schema") != "prismabuild.endings.v1":
                        raise ValueError("PrismaBuild returned an invalid ending")
                    if type(waited.get("complete")) is not bool:
                        raise ValueError("PrismaBuild returned no completion flag")
                    endings = waited["endings"]
                    if len(endings) != 1 or endings[0]["action_key"] != key:
                        raise ValueError("PrismaBuild returned another action")
                    if waited["complete"]:
                        if code:
                            raise ValueError("PrismaBuild action infrastructure failed")
                        result = _summary(endings[0], key)
                        if base:
                            result["base_sha"] = base_sha
                        return result
                    if endings[0]["status"] not in ("waiting", "executed", "cache_hit"):
                        raise ValueError("PrismaBuild could not verify completion")
        except (OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError):
            result = infrastructure("PrismaBuild submission, execution or verified result retrieval failed.")
            if key:
                result["action_key"] = key
            return result

    return run_tests
