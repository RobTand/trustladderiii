"""Trusted generator action. Only the configured command runs in the container."""
import base64
import hashlib
import json
from pathlib import Path, PurePosixPath
import re
import shlex
import shutil
import stat
import subprocess
import sys
import tempfile

from .execution import Container, container_command as test_container_command, require_admission
from .trees import git


PREFIX = "TL3_GENERATOR_ARTIFACT "
FILE_BYTES = 16 * 1_048_576
OUTPUT_BYTES = 32 * 1_048_576
ARTIFACT_BYTES = 48 * 1_048_576
MAX_OUTPUTS = 128
INPUT_BYTES = 512 * 1_048_576
TIMEOUT = 120


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
        if name not in files:
            raise ValueError(f"generator output is outside the footprint: {name}")
        if name not in after:
            raise ValueError(f"generator deleted an output: {name}")
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
    # /dev/shm can be noexec. Invoke a repository script through its declared
    # shebang interpreter while keeping its bytes and arguments unchanged.
    executable = tree / argv[0]
    if ("/" in argv[0] and not Path(argv[0]).is_absolute() and executable.is_file()
            and executable.stat().st_mode & 0o111):
        with executable.open("rb") as stream:
            header = stream.readline(4096)
        if header.startswith(b"#!"):
            argv = [*shlex.split(header[2:].decode().strip()), argv[0], *argv[1:]]
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


def run(tree, container, command, generated_files, protected_paths, files, timeout=TIMEOUT):
    require_admission()
    argv = shlex.split(command)
    if not argv or not generated_files or not 0 < timeout <= TIMEOUT:
        raise ValueError("generator requires a command, generated output patterns and a bounded timeout")
    before = manifest(tree)
    if sum(entry["bytes"] for entry in before.values()) > INPUT_BYTES:
        raise ValueError("generator input exceeds the 512-MiB snapshot limit")
    result = {"status": "failed", "exit_code": None, "message": "Generator did not complete.",
              "input_manifest": digest(before), "command": command,
              "identity": {"implementation": "tl3-generator-v1", "image": container.image}}
    # Writable scratch is memory-backed and charged to the admitted scope,
    # so a generator cannot fill the worker disk before diff validation.
    with tempfile.TemporaryDirectory(prefix="tl3-generator-", dir="/dev/shm") as temporary:
        root = Path(temporary)
        root.chmod(0o755)
        copy = root / "checkout"
        shutil.copytree(tree, copy)
        # The action never receives the served repository's Git directory. The
        # synthetic index includes new files and has no hooks, remotes or secrets.
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
        with tempfile.TemporaryFile(dir=root) as log:
            try:
                child = subprocess.run(container_command(copy, container, argv, cidfile), cwd=copy,
                                       stdout=log, stderr=subprocess.STDOUT, timeout=timeout)
                result["exit_code"] = child.returncode
                if child.returncode:
                    result["message"] = f"Generator exited with status {child.returncode}."
                else:
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
                    result.update(status="completed", message="Generator completed.", outputs=outputs,
                                  output_manifest=digest(after))
            except subprocess.TimeoutExpired:
                result.update(exit_code=124, message=f"Generator exceeded its {timeout}-second deadline.")
            except UnicodeDecodeError:
                result["message"] = "Generator output must be UTF-8 text."
            except (ValueError, OSError) as error:
                result["status"] = "refused"
                result["message"] = str(error)[:1000]
            finally:
                if cidfile.exists():
                    cid = cidfile.read_text().strip()
                    if re.fullmatch(r"[0-9a-f]{64}", cid):
                        subprocess.run(["docker", "rm", "--force", cid], stdout=subprocess.DEVNULL,
                                       stderr=subprocess.DEVNULL, timeout=10)
            log.seek(0)
            result["log"] = log.read(3072).decode(errors="replace")
    return result


def main():
    try:
        contract = json.loads(Path("generator.json").read_text())
        result = run(Path("checkout").resolve(), Container(sys.argv[1], exec_scratch="--exec-scratch" in sys.argv),
                     **contract)
    except (OSError, RuntimeError, ValueError):
        result = {"status": "failed", "exit_code": None, "message": "Generator action infrastructure failed."}
    print(PREFIX + json.dumps(result), flush=True)


if __name__ == "__main__":
    main()
