"""Trusted generator action. Only the configured command runs in the container."""
import base64
from contextlib import contextmanager
import hashlib
import json
from pathlib import Path, PurePosixPath
import re
import shlex
import shutil
import signal
import stat
import subprocess
import sys
import tempfile
import time

from .execution import Container, container_command as test_container_command, require_admission
from .trees import git


PREFIX = "TL3_GENERATOR_ARTIFACT "
FILE_BYTES = 16 * 1_048_576
OUTPUT_BYTES = 32 * 1_048_576
ARTIFACT_BYTES = 48 * 1_048_576
MAX_OUTPUTS = 128
INPUT_BYTES = 512 * 1_048_576
TIMEOUT = 120
MAX_TIMEOUT = 600
PREPARATION_SECONDS = 25
VALIDATION_SECONDS = 20
CLEANUP_SECONDS = 10
# Five more seconds let the trusted wrapper serialize its bounded artifact.
OUTER_OVERHEAD_SECONDS = PREPARATION_SECONDS + VALIDATION_SECONDS + CLEANUP_SECONDS + 5
IMPLEMENTATION = "tl3-generator-v4"


def budget(cpus=1, mem_gb=2, timeout_s=TIMEOUT):
    if (type(cpus) is not int or not 1 <= cpus <= 16
            or type(mem_gb) is not int or not 1 <= mem_gb <= 64
            or type(timeout_s) is not int or not 1 <= timeout_s <= MAX_TIMEOUT):
        raise ValueError("generator budget requires 1–16 CPUs, 1–64 GiB and generator_timeout_s in 1–600")
    return {"cpus": cpus, "mem_gb": mem_gb, "timeout_s": timeout_s,
            "outer_timeout_s": timeout_s + OUTER_OVERHEAD_SECONDS}


class StageTimeout(TimeoutError):
    pass


@contextmanager
def _phase(result, name, allowance=None):
    started = time.monotonic()
    timing = result["timings"][name]
    outcome = "completed"
    previous = None
    if allowance is not None:
        # The trusted action runs in its own main process, outside the container.
        # Bound filesystem/Git preparation and validation as well as docker run.
        def expired(signum, frame):
            raise StageTimeout(name)
        handler = signal.signal(signal.SIGALRM, expired)
        previous = signal.setitimer(signal.ITIMER_REAL, allowance)
    try:
        yield
    except (StageTimeout, subprocess.TimeoutExpired):
        outcome = "timeout"
        raise
    except BaseException:
        outcome = "failed"
        raise
    finally:
        if previous is not None:
            signal.setitimer(signal.ITIMER_REAL, 0)
            signal.signal(signal.SIGALRM, handler)
            if previous[0]:
                signal.setitimer(signal.ITIMER_REAL, max(0.000001, previous[0] - (time.monotonic() - started)), previous[1])
        timing.update(seconds=time.monotonic() - started, outcome=outcome)



def _check_interpreter(argv):
    if not argv or not argv[0]:
        raise ValueError("Use an explicit configured image interpreter, such as python3 tools/bazel/build_files.py")
    executable = PurePosixPath(argv[0])
    if ((not executable.is_absolute() and ("/" in argv[0] or argv[0] in (".", "..")))
            or (executable.is_absolute() and (str(executable) != argv[0] or ".." in executable.parts
                                              or any(executable.is_relative_to(root) for root in
                                                     ("/checkout", "/proc", "/sys", "/dev", "/tmp"))))):
        raise ValueError("Use an explicit configured image interpreter, such as python3 tools/bazel/build_files.py")


def configured_argv(command):
    argv = shlex.split(command)
    _check_interpreter(argv)
    return argv


def digest(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True, separators=(",", ":")).encode()).hexdigest()


def path_parts(name):
    path = PurePosixPath(name)
    if (not name or path.is_absolute() or str(path) != name or
            any(part in ("..", ".git") for part in path.parts) or
            any(ord(char) < 32 or char == "\\" for char in name) or
            len(name.encode()) > 3072 or any(len(part.encode()) > 255 for part in path.parts)):
        raise ValueError("generator path must be a canonical relative file path")
    if path.name == ".tl3-generator-import":
        raise ValueError("reserved generator import path")
    return path.parts


def manifest(root):
    """Hash every regular file, refusing entries a Git/file snapshot would omit."""
    result = {}

    def walk(directory, prefix=""):
        for path in sorted(directory.iterdir()):
            name = prefix + path.name
            if name == ".git":
                continue
            path_parts(name)
            info = path.lstat()
            if stat.S_ISDIR(info.st_mode):
                walk(path, name + "/")
            elif stat.S_ISREG(info.st_mode) and info.st_nlink == 1:
                with path.open("rb") as stream:
                    sha = hashlib.file_digest(stream, "sha256").hexdigest()
                result[name] = {"sha256": sha, "bytes": info.st_size,
                                "mode": 0o755 if info.st_mode & 0o100 else 0o644}
                if len(result) > 100_000:
                    raise ValueError("generator input file count exceeds limit")
            else:
                raise ValueError(f"generator requires regular files: {name}")
    walk(Path(root))
    return result


def changed(before, after, generated_files, protected_paths, files):
    """Authorize trusted outputs from configuration; files still binds legacy action identity."""
    names = sorted(name for name in before.keys() | after.keys() if before.get(name) != after.get(name))
    if len(names) > MAX_OUTPUTS:
        raise ValueError("generator output file count exceeds limit")
    for name in names:
        path_parts(name)
        path = PurePosixPath(name)
        if any(path.full_match(pattern) for pattern in protected_paths):
            raise ValueError(f"generator changed protected output: {name}")
        if not any(path.full_match(pattern) for pattern in generated_files):
            raise ValueError(f"generator changed undeclared generated output: {name}")
        if name not in after:
            raise ValueError(f"generator deleted an output: {name}")
        expected_mode = before[name]["mode"] if name in before else 0o644
        if after[name]["mode"] != expected_mode:
            raise ValueError(f"generator changed an output mode: {name}; new outputs require mode 0644")
    return names


def text(raw):
    if len(raw) > FILE_BYTES:
        raise ValueError("generator output exceeds the file size limit")
    raw.decode("utf-8")
    if any(byte < 32 and byte not in (9, 10, 13) for byte in raw):
        raise ValueError("generator output must be text")


def validate_artifact(artifact, before, contract):
    """Validate the complete diff again before the maker imports a CAS artifact."""
    if artifact["status"] != "completed" or artifact["exit_code"] != 0:
        raise ValueError(artifact["message"])
    if artifact["input_manifest"] != digest(before):
        raise ValueError("generator artifact has a stale input tree")
    if artifact["command"] != contract["command"] or artifact.get("argv") != configured_argv(contract["command"]):
        raise ValueError("generator artifact argv differs from its configured command")
    after = dict(before)
    outputs = artifact["outputs"]
    if not isinstance(outputs, list) or len(outputs) > MAX_OUTPUTS:
        raise ValueError("generator output file count exceeds limit")
    total, seen = 0, set()
    for output in outputs:
        name = output["path"]
        path_parts(name)
        if name in seen:
            raise ValueError("duplicate generator output")
        seen.add(name)
        raw = base64.b64decode(output["data"], validate=True)
        total += len(raw)
        if total > OUTPUT_BYTES:
            raise ValueError("generator output exceeds total size limit")
        text(raw)
        entry = output["entry"]
        if (entry["sha256"] != hashlib.sha256(raw).hexdigest() or entry["bytes"] != len(raw)
                or entry["mode"] not in (0o644, 0o755)):
            raise ValueError("generator output digest or mode mismatch")
        after[name] = entry
    if set(changed(before, after, **{key: contract[key] for key in
                                  ("generated_files", "protected_paths", "files")})) != seen:
        raise ValueError("generator artifact contains an unchanged output")
    if digest(after) != artifact["output_manifest"]:
        raise ValueError("generator output manifest digest mismatch")
    return after


def container_command(tree, container, argv, cidfile):
    # Only trusted configuration supplies the image interpreter and arguments.
    # Never read an entrypoint or shebang from the maker's writable checkout.
    _check_interpreter(argv)
    command = test_container_command(tree, container, ())
    # Preserve every test containment flag and PB's Docker shim/affinity. Only
    # the private copy is writable; Git's synthetic metadata stays read-only.
    mount = f"type=bind,src={tree},dst=/checkout,readonly"
    command[command.index(mount)] = f"type=bind,src={tree},dst=/checkout"
    end = command.index("--entrypoint=python3")
    return command[:end] + [
        "--mount", f"type=bind,src={tree / '.git'},dst=/checkout/.git,readonly",
        "--cidfile", str(cidfile), "--ulimit", f"fsize={OUTPUT_BYTES}:{OUTPUT_BYTES}",
        "--env", "PYTHONDONTWRITEBYTECODE=1",
        "--env", "GIT_CONFIG_COUNT=1", "--env", "GIT_CONFIG_KEY_0=safe.directory",
        "--env", "GIT_CONFIG_VALUE_0=/checkout",
        "--env", "GIT_CONFIG_NOSYSTEM=1", "--env", "GIT_CONFIG_GLOBAL=/dev/null",
        "--env", "GIT_TERMINAL_PROMPT=0", "--env", "GIT_NO_LAZY_FETCH=1",
        "--entrypoint=" + argv[0], container.image, *argv[1:],
    ]


def run(tree, container, command, generated_files, protected_paths, files, timeout=TIMEOUT,
        cpus=1, mem_gb=2):
    require_admission()
    argv = configured_argv(command)
    # Fractional deadlines remain useful to trusted qualification callers. Repo
    # configuration goes through budget(), which accepts whole seconds only.
    if (isinstance(timeout, bool) or not isinstance(timeout, (int, float))
            or not 0 < timeout <= MAX_TIMEOUT or not argv or not generated_files):
        raise ValueError("generator requires a command, generated output patterns and a bounded timeout")
    selected = budget(cpus, mem_gb, max(1, int(timeout)))
    selected.update(timeout_s=timeout, outer_timeout_s=timeout + OUTER_OVERHEAD_SECONDS)
    started = time.monotonic()
    result = {"status": "failed", "exit_code": None, "message": "Generator did not complete.",
              "command": command, "argv": argv, "phase": "preparation", "budget": selected,
              "identity": {"implementation": IMPLEMENTATION, "image": container.image, "budget": selected},
              "timings": {name: {"seconds": 0.0, "outcome": "skipped"} for name in
                          ("preparation", "container", "validation", "cleanup")}}
    result["timings"]["container"]["scope"] = "startup_and_command"
    temporary, log, cidfile = None, None, None
    try:
        with _phase(result, "preparation", PREPARATION_SECONDS):
            before = manifest(tree)
            result["input_manifest"] = digest(before)
            if sum(entry["bytes"] for entry in before.values()) > INPUT_BYTES:
                raise ValueError("generator input exceeds the 512-MiB snapshot limit")
            # Memory-backed scratch stays charged to the admitted scope.
            temporary = tempfile.TemporaryDirectory(prefix="tl3-generator-", dir="/dev/shm")
            root = Path(temporary.name)
            root.chmod(0o755)
            copy = root / "checkout"
            shutil.copytree(tree, copy)
            if (copy / ".git").exists():
                raise ValueError("generator input must not contain Git metadata")
            git(copy, "init", "--quiet", "--template=")
            git(copy, "add", "--force", "--all")
            git(copy, "config", "safe.directory", "/checkout")
            for path in [copy, *copy.rglob("*")]:
                if ".git" in path.relative_to(copy).parts:
                    continue
                path.chmod(0o777 if path.is_dir() else 0o666 | (path.stat().st_mode & 0o111))
            cidfile = root / "container.id"
            log = tempfile.TemporaryFile(dir=root)
        result["phase"] = "container"
        with _phase(result, "container"):
            child = subprocess.run(container_command(copy, container, argv, cidfile), cwd=copy,
                                   stdout=log, stderr=subprocess.STDOUT, timeout=timeout)
            result["exit_code"] = child.returncode
            if child.returncode:
                result["message"] = f"Generator exited with status {child.returncode}."
        if child.returncode:
            result["timings"]["container"]["outcome"] = "failed"
        if child.returncode == 0:
            result["phase"] = "validation"
            with _phase(result, "validation", VALIDATION_SECONDS):
                after = manifest(copy)
                names = changed(before, after, generated_files, protected_paths, files)
                outputs, total = [], 0
                for name in names:
                    with (copy / name).open("rb") as stream:
                        raw = stream.read(FILE_BYTES + 1)
                    text(raw)
                    total += len(raw)
                    if total > OUTPUT_BYTES:
                        raise ValueError("generator output exceeds total size limit")
                    outputs.append({"path": name, "entry": after[name],
                                    "data": base64.b64encode(raw).decode("ascii")})
                result.update(status="completed", phase="completed", message="Generator completed.",
                              outputs=outputs, output_manifest=digest(after))
    except subprocess.TimeoutExpired:
        result.update(exit_code=124, message=f"Generator exceeded its {timeout}-second deadline.")
    except StageTimeout:
        result.update(exit_code=124, message=f"Generator {result['phase']} exceeded its stage deadline.")
    except UnicodeDecodeError:
        result["message"] = "Generator output must be UTF-8 text."
    except ValueError as error:
        result.update(status="refused", message=str(error)[:1000])
    except (OSError, RuntimeError, subprocess.SubprocessError):
        result["message"] = "Generator action infrastructure failed."
    finally:
        try:
            with _phase(result, "cleanup", CLEANUP_SECONDS):
                if cidfile is not None and cidfile.exists():
                    cid = cidfile.read_text().strip()
                    if re.fullmatch(r"[0-9a-f]{64}", cid):
                        subprocess.run(["docker", "rm", "--force", cid], check=True,
                                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=CLEANUP_SECONDS)
                if log is not None:
                    log.seek(0)
                    result["log"] = log.read(3072).decode(errors="replace")
                    log.close()
                if temporary is not None:
                    temporary.cleanup()
        except (OSError, ValueError, RuntimeError, StageTimeout, subprocess.SubprocessError):
            if result["status"] == "completed":
                result.update(status="failed", phase="cleanup", message="Generator cleanup failed.")
        result["timings"]["total_seconds"] = time.monotonic() - started
    return result


def main():
    try:
        contract = json.loads(Path("generator.json").read_text())
        result = run(Path("checkout").resolve(), Container(sys.argv[1]),
                     **contract)
    except (OSError, RuntimeError, ValueError):
        result = {"status": "failed", "exit_code": None, "message": "Generator action infrastructure failed."}
    print(PREFIX + json.dumps(result), flush=True)


if __name__ == "__main__":
    main()
