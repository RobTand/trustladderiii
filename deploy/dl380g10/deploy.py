#!/usr/bin/env python3
"""Install and inspect TL3 on dl380g10 without printing credentials."""
import argparse
import importlib.util
from datetime import datetime, timezone
import fnmatch
import hashlib
import http.client
import io
import json
import os
from pathlib import Path
import re
import secrets
import select
import signal
import shutil
import socket
import stat
import subprocess
import sys
import tarfile
import tempfile
import time
import tomllib
import urllib.request
import zipfile

SOURCE = Path(__file__).resolve().parents[2]
HERE = Path(__file__).resolve().parent
PG_VERSION = "17.4.0"
PG_URL = ("https://repo1.maven.org/maven2/io/zonky/test/postgres/"
          f"embedded-postgres-binaries-linux-amd64/{PG_VERSION}/"
          f"embedded-postgres-binaries-linux-amd64-{PG_VERSION}.jar")
PG_SHA256 = "d9d216d3c1c119ad31b8a8de60b3cf2826516f711a04d3745ef4f1913f21a938"
PG_EXECUTABLES = ("initdb", "pg_ctl", "postgres")
TOKENIZER_URL = "https://openaipublic.blob.core.windows.net/encodings/o200k_base.tiktoken"
TOKENIZER_SHA256 = "446a9538cb6c348e3516120d7c08b09f57c36495e2acfffe59a5bf8b0cfb1a2d"
TOKENIZER_KEY = hashlib.sha1(TOKENIZER_URL.encode()).hexdigest()
# #372: each attempt's supervisor runs in its own scope under this slice, outside
# tl3d's cgroup, so restarting or deploying tl3d leaves runs alive.
WORKER_SLICE = "tl3-workers.slice"
LAUNCHER = ["/usr/bin/systemd-run", "--user", "--scope", "--quiet", "--collect", f"--slice={WORKER_SLICE}"]
RELEASE_BINARIES = ("tl3", "tl3d", "tl3-deploy", "tl3-worker-supervisor")
RELEASE_PACKAGES = ("workers", "harness")
RELEASE_IGNORED = (".venv", "__pycache__", ".pytest_cache", "*.egg-info")


def paths(home):
    root = home / ".local/share/tl3"
    return root, home / ".config/tl3", root / f"postgresql-{PG_VERSION}"


def private_dir(path):
    path.mkdir(parents=True, exist_ok=True, mode=0o700)
    if path.is_symlink() or path.stat().st_uid != os.getuid():
        raise RuntimeError("deployment directory must belong to this user and not be a symlink")
    path.chmod(0o700)


def private_file(path):
    info = path.lstat()
    if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or stat.S_IMODE(info.st_mode) != 0o600:
        raise RuntimeError("credential must be a regular owner-only file with mode 0600")
    if info.st_size == 0:
        raise RuntimeError("credential must not be empty")


def sync_directory(path):
    descriptor = os.open(path, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def write_file(path, data, mode=0o600):
    private_dir(path.parent)
    fd, temporary = tempfile.mkstemp(prefix=".tl3-", dir=path.parent)
    try:
        with os.fdopen(fd, "wb") as output:
            os.fchmod(output.fileno(), mode)
            output.write(data)
            output.flush()
            os.fsync(output.fileno())
        os.replace(temporary, path)
        sync_directory(path.parent)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


def password(path):
    if path.exists() or path.is_symlink():
        private_file(path)
        return
    private_dir(path.parent)
    # Publish a complete credential without overwriting an earlier run's file.
    with tempfile.TemporaryDirectory(prefix=".password-", dir=path.parent) as directory:
        temporary = Path(directory) / "password"
        write_file(temporary, (secrets.token_hex(32) + "\n").encode())
        try:
            os.link(temporary, path)
            sync_directory(path.parent)
        except FileExistsError:
            private_file(path)


def checked_download(url, digest, target):
    private_dir(target.parent)
    if target.exists() and hashlib.sha256(target.read_bytes()).hexdigest() == digest:
        return
    with tempfile.TemporaryDirectory(prefix="download-", dir=target.parent) as directory:
        download = Path(directory) / "payload"
        with urllib.request.urlopen(url, timeout=60) as response, download.open("wb") as output:
            shutil.copyfileobj(response, output)
        if hashlib.sha256(download.read_bytes()).hexdigest() != digest:
            raise RuntimeError("download SHA-256 mismatch")
        os.replace(download, target)


def pg_executable(binaries, name):
    if name not in PG_EXECUTABLES:
        raise RuntimeError("executable is outside the pinned Postgres package")
    return binaries / "bin" / name


def verify_postgres_executables(binaries):
    for name in PG_EXECUTABLES:
        executable = pg_executable(binaries, name)
        if not executable.is_file() or not os.access(executable, os.X_OK):
            raise RuntimeError("incomplete versioned Postgres installation")


def postgres(home):
    root, config, binaries = paths(home)
    private_dir(root)
    if not binaries.exists():
        with tempfile.TemporaryDirectory(prefix="postgres-download-", dir=root) as directory:
            directory = Path(directory)
            archive = directory / "postgres.jar"
            checked_download(PG_URL, PG_SHA256, archive)
            with zipfile.ZipFile(archive) as jar:
                archives = [name for name in jar.namelist() if name.endswith(".txz")]
                if len(archives) != 1:
                    raise RuntimeError("unexpected Zonky archive")
                with tarfile.open(fileobj=io.BytesIO(jar.read(archives[0])), mode="r:xz") as tar:
                    tar.extractall(directory / "unpacked", filter="data")
            verify_postgres_executables(directory / "unpacked")
            os.rename(directory / "unpacked", binaries)
    verify_postgres_executables(binaries)
    credential = config / "secrets/postgres-password"
    if (root / "postgres/data").exists() and not credential.exists():
        raise RuntimeError("existing Postgres data needs its original password file; refusing to generate another")
    password(credential)
    password(config / "secrets/console-postgres-password")
    private_dir(root / "postgres")
    private_dir(root / "postgres/socket")
    print(f"Postgres: Zonky {PG_VERSION} installed")


def initdb(home):
    root, config, binaries = paths(home)
    data = root / "postgres/data"
    credential = (Path(os.environ["CREDENTIALS_DIRECTORY"]) / "postgres-password"
                  if "CREDENTIALS_DIRECTORY" in os.environ else config / "secrets/postgres-password")
    if "CREDENTIALS_DIRECTORY" not in os.environ:
        private_file(credential)
    if data.exists():
        if (data / "PG_VERSION").read_text().strip() != "17":
            raise RuntimeError("existing cluster major version is unsupported; refusing cluster modification")
        return
    with tempfile.TemporaryDirectory(prefix="initdb-", dir=data.parent) as directory:
        stage = Path(directory) / "data"
        run([pg_executable(binaries, "initdb"), "-D", stage, "-U", "tl3", "--encoding=UTF8",
             "--locale=C", "--auth=scram-sha-256", f"--pwfile={credential}"])
        socket_dir = str(root / "postgres/socket").replace("'", "''")
        with (stage / "postgresql.conf").open("a") as output:
            output.write(f"\nlisten_addresses = ''\nport = 5543\nunix_socket_directories = '{socket_dir}'\n"
                         "unix_socket_permissions = 0700\nfsync = on\nsynchronous_commit = on\n"
                         "max_connections = 30\nshared_buffers = '128MB'\nmax_parallel_workers = 0\n")
            output.flush()
            os.fsync(output.fileno())
        os.rename(stage, data)
        sync_directory(data.parent)


def lan_address(listen, service):
    address, port = listen.rsplit(":", 1)
    socket.inet_pton(socket.AF_INET, address)
    if address.startswith("127.") or address == "0.0.0.0" or not 0 < int(port) < 65536:
        raise RuntimeError(f"{service} must use the LAN interface address and a fixed port")
    return address


def console_fields(home, listen, console_listen=None):
    """The console listens beside the status page unless told otherwise, and
    reads Postgres as its own login that may only read."""
    console_listen = console_listen or lan_address(listen, "status") + ":8034"
    lan_address(console_listen, "console")
    root, config, _ = paths(home)
    return {"listen": console_listen, "assets": str(root / "console"), "user": "tl3_console",
            "password_file": str(config / "secrets/console-postgres-password")}


def render_config(home, listen, installation=1, issue=None, image=None, primary=None, alternate=None,
                  console_listen=None):
    lan_address(listen, "status")
    console = console_fields(home, listen, console_listen)
    root, config, _ = paths(home)
    text = (SOURCE / "crates/config/example.toml").read_text()
    start = text.index('# Names below are deployment placeholders')
    end = text.index('\n[prismabuild]', start)
    models = (HERE / "models.toml").read_text().removeprefix('[models]\n')
    text = text[:start] + models + text[end:]
    replacements = {
        '"/srv/tl3/worktrees"': json.dumps(str(root / "worktrees")),
        'host = "127.0.0.1"': "host = " + json.dumps(str(root / "postgres/socket")),
        'port = 5432': 'port = 5543',
        '# password_file = "/run/tl3/postgres-password"': "password_file = " + json.dumps(str(config / "secrets/postgres-password")),
        '"/home/rob/.config/tl3/github-app.json"': json.dumps(str(config / "github-app.json")),
        '# api_key_file = "/run/tl3/hindsight-key"': "api_key_file = " + json.dumps(str(config / "secrets/hindsight-key")),
        '"/run/tl3/model-key"': json.dumps(str(config / "secrets/model-key")),
        'installation_id = 1': f'installation_id = {installation}',
        'listen = "127.0.0.1:8033"': f'listen = {json.dumps(listen)}',
        'listen = "127.0.0.1:8034"': f'listen = {json.dumps(console["listen"])}',
        '"/srv/tl3/console"': json.dumps(console["assets"]),
        '"/srv/tl3/workers/.venv/bin/python"': json.dumps(str(root / "current/workers/.venv/bin/python")),
        '"/srv/tl3/worker-runs"': json.dumps(str(root / "worker-runs")),
        '# tiktoken_cache_dir = "/srv/tl3/tiktoken"': "tiktoken_cache_dir = " + json.dumps(str(root / "tiktoken")),
    }
    if image:
        replacements['sha256:' + '0' * 64] = image
    # Primary is maker attempt 1; alternate is attempt 2 onward (#416). Both rename every role that uses them.
    if primary:
        replacements['claude-sonnet-5-5'] = primary
    if alternate:
        replacements['claude-opus-5-5'] = alternate
    for old, new in replacements.items():
        text = text.replace(old, new)
    text = text.replace('[github]\n', '[github]\nwebhook_listen = "127.0.0.1:8081"\n')
    if issue is not None:
        text = text.replace('[github]\n', '[github]\noperations = ' + toml_value({"owner": "RobTand", "name": "trustladderiii", "number": issue, "installation_id": installation}) + '\n')
        text += f'\n[operations]\nowner = "RobTand"\nrepo = "trustladderiii"\nissue = {issue}\n'
    text = add_fields(text, "[console]", {key: toml_value(console[key]) for key in ("user", "password_file")})
    text = add_fields(text, "[workers]", {"launcher": toml_value(LAUNCHER)})
    tomllib.loads(text)
    return text


def add_fields(text, table, fields):
    """Insert missing fields without reserializing operator settings or comments."""
    header = re.search(r"(?m)^" + re.escape(table) + r"\s*(?:#.*)?$", text)
    additions = "".join(f"{key} = {value}\n" for key, value in fields.items())
    if header:
        # Insert directly under the header, so every other line stays byte-identical.
        return text[:header.end()] + "\n" + additions.rstrip("\n") + text[header.end():]
    return text + "\n" + table + "\n" + additions


def configure(home, listen, state, image=None, primary=None, alternate=None, console_listen=None):
    _, config, _ = paths(home)
    destination = config / "tl3.toml"
    defaults = render_config(home, listen, state.get("installation_id", 1),
                             state.get("operations_issue"), image, primary, alternate, console_listen)
    if not destination.exists():
        write_file(destination, defaults.encode())
        return
    ignored = [flag for flag, value in (("--image", image), ("--primary", primary), ("--alternate", alternate))
               if value]
    if ignored:
        print(f"Config exists, so {', '.join(ignored)} had no effect; edit {destination} instead.")
    original = destination.read_text()
    data = tomllib.loads(original)
    if not data.get("github", {}).get("repos"):
        raise SystemExit(f"{destination} has no [[github.repos]] entry. Restore it from a "
                         "tl3.toml.*.bak backup, or move the file aside to generate a new one.")
    # Existing configuration is operator input, never an upgrade target.
    required = {"workers": ("python", "runs", "launcher", "tiktoken_cache_dir"),
                "hindsight": ("bank", "api_key_file"), "models": ("writer",),
                "github": ("webhook_listen", "operations"),
                "console": ("listen", "assets", "user", "password_file")}
    for table, fields in required.items():
        if any(field not in data.get(table, {}) for field in fields):
            raise RuntimeError("existing config lacks exact current deployment inputs; edit it explicitly")
    if data["workers"]["python"] == str(paths(home)[0] / "app/workers/.venv/bin/python"):
        raise RuntimeError("existing config must select the prepared immutable runtime explicitly")


def save_config(destination, original, text):
    if text != original:
        backup = destination.with_name("tl3.toml." + datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%S.%fZ") + ".bak")
        write_file(backup, original.encode())
        write_file(destination, text.encode())
        print(f"Config: updated; 0600 backup {backup.name}")


def model_defaults(home):
    """Explicit operator command; ordinary installation never replaces ladders."""
    destination = paths(home)[1] / "tl3.toml"
    original = destination.read_text()
    data = tomllib.loads(original)["models"]
    defaults = tomllib.loads((HERE / "models.toml").read_text())["models"]
    data.update(defaults)
    section = "[models]\n" + "".join(f"{key} = {toml_value(value)}\n" for key, value in data.items())
    text = re.sub(r"(?ms)^\[models\].*?(?=^\[|\Z)", lambda _: section + "\n", original)
    tomllib.loads(text)
    save_config(destination, original, text)


def toml_value(value):
    if isinstance(value, dict):
        return "{ " + ", ".join(f"{key} = {toml_value(item)}" for key, item in value.items()) + " }"
    if isinstance(value, list):
        return "[" + ", ".join(map(toml_value, value)) + "]"
    return json.dumps(value)


def run(command, **kwargs):
    # Commands never carry secret values. Suppress output on failure as well.
    timeout = kwargs.pop("timeout", 300)
    input_bytes = kwargs.pop("input", None)
    if input_bytes is not None:
        kwargs["stdin"] = subprocess.PIPE
    with subprocess.Popen([str(part) for part in command], stdout=subprocess.PIPE,
                          stderr=subprocess.PIPE, start_new_session=True, **kwargs) as child:
        try:
            output, _ = child.communicate(input=input_bytes, timeout=timeout)
        except subprocess.TimeoutExpired:
            os.killpg(child.pid, signal.SIGKILL)
            child.communicate(timeout=5)
            raise RuntimeError(f"{Path(command[0]).name} timed out; output suppressed") from None
        if child.returncode:
            raise RuntimeError(f"{Path(command[0]).name} failed with exit {child.returncode}; output suppressed")
        return output


def unit_state(unit):
    return run(["systemctl", "--user", "show", unit, "--property=ActiveState", "--value"],
               timeout=15).decode().strip()


def require_stopped(*units):
    for unit in units or ("tl3d.service", "tl3-postgres.service"):
        if unit_state(unit) != "inactive":
            raise RuntimeError(f"{unit} must be exactly stopped (inactive)")


def owned_directory(path):
    path = Path(path)
    if not path.is_absolute() or any(parent.is_symlink() for parent in (path, *path.parents)):
        raise RuntimeError("state/backup path must be absolute with no symlink ancestors")
    info = path.stat()
    if not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o077:
        raise RuntimeError("state/backup root must be an owner-only directory")


def tree_manifest(path, links=None):
    """Hash regular files without following links; bind metadata and empty directories."""
    owned_directory(path)
    links = links or {}
    result = {}
    for item in [path, *sorted(path.rglob("*"))]:
        relative = str(item.relative_to(path))
        info = item.lstat()
        row = {"uid": info.st_uid, "mode": stat.S_IMODE(info.st_mode)}
        if info.st_uid != os.getuid() or (not stat.S_ISLNK(info.st_mode) and info.st_mode & 0o022):
            raise RuntimeError("backup source must be owned and not writable by others")
        if stat.S_ISLNK(info.st_mode):
            if item not in links or item.resolve(strict=True) != links[item][0]:
                raise RuntimeError("undeclared symlink in backup source")
            row = {"kind": "tablespace", "target": links[item][1]}
        elif stat.S_ISDIR(info.st_mode):
            row["kind"] = "directory"
        elif stat.S_ISREG(info.st_mode):
            row["kind"] = "file"
            digest = hashlib.sha256()
            with item.open("rb") as stream:
                for block in iter(lambda: stream.read(1024 * 1024), b""):
                    digest.update(block)
            row.update(size=info.st_size, sha256=digest.hexdigest())
        else:
            raise RuntimeError("unsupported special file in backup source")
        result[relative] = row
    return result


def backup_links(sources):
    return {sources["data"] / "pg_tblspc" / name.removeprefix("tablespace-"): (path, name)
            for name, path in sources.items() if name.startswith("tablespace-")}


DEPENDENT_UNITS = ("tl3-mlflow.service", "tl3-status.service", "tl3-console.service")
CLIENT_UNITS = ("tl3d.service", *DEPENDENT_UNITS)
EVIDENCE_ROOT = Path("/home/rob/tmp/tl3-build/rust-fv/captain/evidence")


def validate_backup_destination(backup):
    if not backup.is_absolute() or backup.exists() or backup.is_symlink():
        raise RuntimeError("backup destination must be a new absolute private directory")
    if any(parent.is_symlink() for parent in backup.parents):
        raise RuntimeError("backup destination has a symlink ancestor")
    backup.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    owned_directory(backup.parent)


def preflight_backup(sources, backup):
    validate_backup_destination(backup)
    # Non-database roots can be validated while services still run. The cold
    # boundary repeats these checks after stop; no live database bytes are read.
    for name, path in sources.items():
        if name != "data" and not name.startswith("tablespace-"):
            tree_manifest(path)
    total = sum(item.lstat().st_size for path in sources.values()
                for item in path.rglob("*") if stat.S_ISREG(item.lstat().st_mode))
    # Conservative transfer budget, not a measured throughput claim. At most
    # three complete cluster hashes plus copy/fsync; no full source rehash at reset.
    estimated = 1800 + total * 8 / (20 * 1024 * 1024)
    if estimated > 6900:
        raise RuntimeError("backup inventory exceeds the admitted 7200s action budget; resize before outage")
    print(f"Backup preflight: {total} bytes; budget estimate {estimated:.0f}s of 6900s")


def cold_backup(sources, backup):
    require_stopped()
    validate_backup_destination(backup)
    links = backup_links(sources)
    before = {name: tree_manifest(path, links) for name, path in sources.items()}
    if not (sources["data"] / "PG_VERSION").is_file() or not (sources["data"] / "pg_wal").is_dir():
        raise RuntimeError("backup requires a complete existing Postgres cluster including WAL")
    if (sources["data"] / "postmaster.pid").exists():
        raise RuntimeError("backup cluster still has postmaster.pid; stopped state is not proven")
    backup.mkdir(mode=0o700)
    for name, path in sources.items():
        if not re.fullmatch(r"[a-z][a-z0-9-]*", name):
            raise RuntimeError("invalid backup root name")
        shutil.copytree(path, backup / name, symlinks=True)
    for source, (_, name) in links.items():
        target = backup / "data" / source.relative_to(sources["data"])
        target.unlink()
        target.symlink_to(Path("../../") / name)
    require_stopped()
    write_file(backup / "manifest.json", json.dumps({"schema": "tl3.cold-backup.v1",
               "sources": {name: str(path) for name, path in sources.items()}, "trees": before,
               "stopped": ["tl3d.service", "tl3-postgres.service"]}, sort_keys=True).encode())
    verify_cold_backup(sources, backup, compare_sources=False)
    # Flush the diagnostic copy before making its verified marker durable.
    for item in backup.rglob("*"):
        if item.is_symlink():
            continue
        descriptor = os.open(item, os.O_RDONLY)
        try:
            os.fsync(descriptor)
        finally:
            os.close(descriptor)
    sync_directory(backup)


def verify_cold_backup(sources, backup, compare_sources=True, names=None):
    if compare_sources:
        require_stopped()
    try:
        owned_directory(backup)
        private_file(backup / "manifest.json")
        manifest = json.loads((backup / "manifest.json").read_text())
        copies = {name: backup / name for name in sources}
        if manifest.get("schema") != "tl3.cold-backup.v1" or manifest.get("sources") != {
                name: str(path) for name, path in sources.items()}:
            raise RuntimeError("backup source scope mismatch")
        selected = set(sources) if names is None else set(names)
        if not selected <= sources.keys():
            raise RuntimeError("backup verification scope mismatch")
        for roots in (sources, copies) if compare_sources else (copies,):
            if {name: tree_manifest(roots[name], backup_links(roots)) for name in selected} != {
                    name: manifest["trees"][name] for name in selected}:
                raise RuntimeError("backup incomplete or mismatched")
    except (OSError, ValueError, KeyError) as error:
        raise RuntimeError("backup missing, incomplete or unreadable") from error


def reset_file_state(sources, backup, reset):
    verify_cold_backup(sources, backup, names={Path(name).parts[0] for name in reset})
    targets = []
    for name in reset:
        parts = Path(name).parts
        if (len(parts) not in (1, 2) or any(part in (".", "..", "/") for part in parts)
                or parts[0] in ("data", "config") or parts[0].startswith("tablespace-") or parts[0] not in sources):
            raise RuntimeError("reset must name only manifest-scoped TL3 file state")
        path = sources[parts[0]].joinpath(*parts[1:])
        owned_directory(path)
        targets.append(path)
    for path in targets:
        require_stopped()
        shutil.rmtree(path)
        path.mkdir(mode=0o700)
        sync_directory(path.parent)


def qualify_fresh(args):
    """Reuse the operator's authenticated gate, with an unconditional exact-head nightly."""
    if not re.fullmatch(r"[0-9a-f]{40}", args.head or ""):
        raise RuntimeError("qualification requires a full exact head")
    helper = Path("/home/rob/tmp/tl3-build/dev_evidence.py")
    spec = importlib.util.spec_from_file_location("tl3_deploy_evidence", helper)
    gate = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(gate)
    check_fresh_qualification(gate, args)


def check_fresh_qualification(gate, args):
    try:
        if args.lane == "release":
            if not gate.postmerge_ok(args.head) or gate.commit_inventory(str(SOURCE), gate.BuildBuddy(), args.head):
                raise RuntimeError("release requires authenticated exact-head full CI and kernel inventory")
        else:
            bb = gate.BuildBuddy()
            _, groups = gate.verify_evidence(str(SOURCE), bb, args.head, EVIDENCE_ROOT / f"{args.head}.json")
            if gate.verus_and_kernel_tests(str(SOURCE), bb, args.head, groups):
                raise RuntimeError("exact-head qualification lacks required proof/test coverage")
        proof = gate.nightly_proof(str(SOURCE), gate.fingerprint(str(SOURCE), args.head))
        if not proof or proof[1] != args.head:
            raise RuntimeError("one authenticated matching nightly at the chosen exact head is required")
    except gate.Refuse as error:
        raise RuntimeError("authenticated deployment qualification refused") from error
    print(f"Qualified {args.head}; nightly run {proof[0]}")


def verify_runtime_inputs(flux, plan):
    for locator in flux["runtime_bundles"]:
        manifest = Path(locator)
        bundle = manifest.parent
        if (manifest.name != "manifest.json" or bundle.parent != Path("/mnt/shared/flux-out/tl3/.runtime")
                or not re.fullmatch(r"[0-9a-f]{64}", bundle.name)
                or not manifest.is_file() or manifest.is_symlink()
                or any(parent.is_symlink() for parent in manifest.parents)
                or plan.get("runtime_manifests", {}).get(str(manifest)) != hashlib.sha256(manifest.read_bytes()).hexdigest()):
            raise RuntimeError("missing or mismatched immutable Flux runtime manifest")
        if hashlib.sha256(manifest.read_bytes()).hexdigest() != bundle.name:
            raise RuntimeError("runtime manifest content address mismatch")
        verify_runtime_files(bundle, json.loads(manifest.read_text()))


def verify_runtime_files(bundle, manifest):
    """Check sealed input bytes; runtime qualification remains the authoritative FX gate."""
    if manifest.get("schema") != "tl3.runtime_bundle.v1" or not manifest.get("files"):
        raise RuntimeError("runtime file inventory is missing")
    expected = {"manifest.json"}
    for entry in manifest["files"]:
        relative = Path(entry["path"])
        if relative.is_absolute() or ".." in relative.parts or str(relative) in expected or not relative.parts:
            raise RuntimeError("runtime file inventory path refused")
        path = bundle / relative
        if any(part.is_symlink() for part in (path, *path.parents)):
            raise RuntimeError("runtime file inventory contains a symlink")
        info = path.lstat()
        if (not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_nlink != 1
                or stat.S_IMODE(info.st_mode) != entry["mode"] or info.st_mode & 0o222
                or info.st_size != entry["bytes"]
                or hashlib.sha256(path.read_bytes()).hexdigest() != entry["sha256"]):
            raise RuntimeError("runtime file inventory bytes or metadata mismatch")
        expected.add(str(relative))
    actual = set()
    for path in bundle.rglob("*"):
        if path.is_symlink():
            raise RuntimeError("runtime file inventory contains a symlink")
        if path.is_file():
            actual.add(str(path.relative_to(bundle)))
    if actual != expected:
        raise RuntimeError("runtime file inventory has missing or unlisted inputs")


def fresh_scope(args):
    """Bind the operator's explicit archive/reset manifest to supported config paths."""
    root, config_dir, _ = paths(args.home)
    config = config_dir / "tl3.toml"
    private_file(config)
    private_file(args.state_manifest)
    plan = json.loads(args.state_manifest.read_text())
    data = tomllib.loads(config.read_text())
    if (plan.get("schema") != "tl3.fresh-init.v1" or plan.get("head") != args.head
            or plan.get("config_sha256") != hashlib.sha256(config.read_bytes()).hexdigest()):
        raise RuntimeError("fresh-init manifest must bind exact head and existing config")
    store = data["store"]
    if (store["database"] != "tl3" or store["user"] != "tl3" or store["port"] != 5543
            or store["host"] != str(root / "postgres/socket")):
        raise RuntimeError("fresh-init only resets database tl3 at the configured local TL3 socket")
    if data.get("execution_backend") != "flux":
        raise RuntimeError("fresh-init requires the explicitly selected Flux backend")
    flux = data["flux"]
    if flux != plan.get("flux") or flux["output_root"] != "/mnt/shared/flux-out/tl3":
        raise RuntimeError("fresh-init requires exact existing Flux inputs; no rebinding")
    if not flux["uri"] or not flux["instance_generation"] or not flux["runtime_bundles"]:
        raise RuntimeError("missing exact Flux runtime inputs")
    verify_runtime_inputs(flux, plan)
    owned_directory(args.binaries)
    for name in RELEASE_BINARIES:
        info = (args.binaries / name).lstat()
        if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or stat.S_IMODE(info.st_mode) != 0o700:
            raise RuntimeError("selected release binaries must be private owned regular executables")
    for name in ("SOURCE.json", "SHA256SUMS"):
        info = (args.binaries / name).lstat()
        if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o022:
            raise RuntimeError("selected release metadata must be owned regular files")
    selected = json.loads((args.binaries / "SOURCE.json").read_text())
    if selected.get("head") != args.head or selected.get("binaries") != {
            name: hashlib.sha256((args.binaries / name).read_bytes()).hexdigest() for name in RELEASE_BINARIES}:
        raise RuntimeError("selected release provenance or binary hashes mismatch")
    lines = (args.binaries / "SHA256SUMS").read_text().splitlines()
    expected_sums = {f"{digest}  {name}" for name, digest in selected["binaries"].items()}
    if len(lines) != len(RELEASE_BINARIES) or set(lines) != expected_sums:
        raise RuntimeError("selected release checksum manifest mismatch")
    actual_tree = run(["git", "rev-parse", "HEAD^{tree}"], cwd=SOURCE, timeout=15)
    selected_tree = run(["git", "rev-parse", f"{args.head}^{{tree}}"], cwd=SOURCE, timeout=15)
    if actual_tree.strip() != selected_tree.strip():
        raise RuntimeError("deployment helper checkout must match the selected exact head's tree")
    if run(["git", "status", "--porcelain"], cwd=SOURCE, timeout=15).strip():
        raise RuntimeError("deployment helper checkout must be clean")
    runs = Path(data["workers"]["runs"])
    worktrees = Path(data["worktrees_root"])
    owned_directory(runs)
    allowed = {"controller": runs / ".controller"}
    # Retain attempts as evidence. An inventory authorizes copying, never wiping their parent.
    attempts = plan.get("attempts")
    if not isinstance(attempts, list) or len(set(attempts)) != len(attempts):
        raise RuntimeError("manifest must inventory exact attempt directories")
    actual_attempts = []
    for run_root in runs.iterdir():
        if re.fullmatch(r"(?:[0-9a-f]{2})+", run_root.name):
            if run_root.is_symlink() or not run_root.is_dir():
                raise RuntimeError("attempt run root must be an owned real directory")
            for attempt in run_root.iterdir():
                if attempt.name.startswith("attempt-"):
                    actual_attempts.append(str(attempt.relative_to(runs)))
    if set(attempts) != set(actual_attempts):
        raise RuntimeError("manifest omits or mismatches exact attempt inventory")
    for index, relative in enumerate(attempts):
        parts = Path(relative).parts
        if (len(parts) != 2 or not re.fullmatch(r"(?:[0-9a-f]{2})+", parts[0])
                or not re.fullmatch(r"attempt-[0-9]{8}", parts[1])):
            raise RuntimeError("unsupported exact attempt directory")
        allowed[f"attempt-{index}"] = runs / relative
    active = [runs / ".controller" / name for name in ("flux-checks", "worker-waits")]
    if any(path.is_symlink() or (path.exists() and any(path.iterdir())) for path in active):
        raise RuntimeError("retained FX bindings/waits need separately verified cleanup; fresh-init refuses")
    if any((path / filename).exists() for name, path in allowed.items() if name.startswith("attempt-")
           for filename in ("execution.json", "claim.json")):
        raise RuntimeError("attempt execution bindings need separately verified cleanup; fresh-init refuses")
    archive = plan.get("archive")
    reset = plan.get("reset")
    if not isinstance(archive, dict) or archive != {name: str(path) for name, path in allowed.items()}:
        raise RuntimeError("archive manifest must enumerate exact supported controller journals and attempts")
    resettable = {"controller/" + name for name in ("direct-merge", "workers", "writer", "github", "mergify", "prismabuild")}
    controller = runs / ".controller"
    known = {name.split("/")[1] for name in resettable} | {"flux-checks", "worker-waits"}
    if any(path.name not in known for path in controller.iterdir()):
        raise RuntimeError("unknown controller state requires exact source-contract reconciliation")
    existing = {name for name in resettable if (runs / ".controller" / name.split("/")[1]).exists()}
    if not isinstance(reset, list) or len(set(reset)) != len(reset) or set(reset) != existing:
        raise RuntimeError("reset must inventory exactly the supported existing TL3 adapter journals")
    sources = {"data": root / "postgres/data", "config": config_dir, **allowed}
    tablespaces = plan.get("tablespaces")
    if not isinstance(tablespaces, dict):
        raise RuntimeError("tablespaces must explicitly declare every external cluster target")
    for oid, target in tablespaces.items():
        if not re.fullmatch(r"[1-9][0-9]*", oid):
            raise RuntimeError("invalid declared tablespace identifier")
        sources[f"tablespace-{oid}"] = Path(target)
    for path in sources.values():
        owned_directory(path)
    protected = [Path(flux[key]) for key in ("staging_root", "output_root")]
    protected.append(worktrees)
    protected.extend(Path(path).parent for path in flux["runtime_bundles"])
    values = list(sources.values())
    for index, path in enumerate(values):
        if any(path.is_relative_to(other) or other.is_relative_to(path)
               for other in values[index + 1:] + protected):
            raise RuntimeError("backup/reset scope overlaps another source or retained Flux state")
        if path.is_relative_to(args.backup) or args.backup.is_relative_to(path):
            raise RuntimeError("backup destination overlaps source state")
    return sources, reset, config


def listener_ready(address):
    host, port = address.rsplit(":", 1)
    connection = http.client.HTTPConnection(host, int(port), timeout=2)
    try:
        connection.request("GET", "/github/events")
        return connection.getresponse().status == 405
    except OSError:
        return False
    finally:
        connection.close()


def fresh_init(args):
    # Qualification runs in a bounded child, including the authoritative helper's I/O.
    run([sys.executable, "-I", HERE / "deploy.py", "qualify-fresh", "--head", args.head,
         "--lane", args.lane], timeout=600)
    sources, reset, config = fresh_scope(args)
    preflight_backup(sources, args.backup)
    helper = args.binaries / "tl3-deploy"
    run([helper, "validate", config], timeout=30)
    marker = config.parent / "production-ready"
    if marker.is_symlink() or not marker.is_file() or marker.stat().st_uid != os.getuid():
        raise RuntimeError("the existing production readiness marker is required before fresh-init")
    # Refuse an incomplete selected bundle before the outage.
    for binary in RELEASE_BINARIES:
        if not (args.binaries / binary).is_file():
            raise RuntimeError("selected release is incomplete")
    run(["systemctl", "--user", "stop", "tl3d.service"], timeout=150)
    require_stopped("tl3d.service")
    for unit in DEPENDENT_UNITS:
        run(["systemctl", "--user", "stop", unit], timeout=150)
    require_stopped(*CLIENT_UNITS)
    run(["systemctl", "--user", "stop", "tl3-postgres.service"], timeout=150)
    require_stopped(*CLIENT_UNITS, "tl3-postgres.service")
    cold_backup(sources, args.backup)
    saved_scope = args.backup / "state-scope.json"
    write_file(saved_scope, args.state_manifest.read_bytes())
    args.state_manifest = saved_scope
    reset_file_state(sources, args.backup, reset)
    context = args.backup / "reset-context.json"
    # A token exists only in this orchestrator and private child pipes. An old
    # backup/context alone cannot authorize DROP, even for the same cluster.
    capability = secrets.token_hex(32).encode()
    fields = {key: str(getattr(args, key)) for key in
              ("home", "head", "lane", "binaries", "state_manifest", "backup")}
    fields.update(capability_sha256=hashlib.sha256(capability).hexdigest(),
                  expires_monotonic=time.monotonic() + 3600,
                  system_identifier=cold_cluster_identity(helper, args.backup))
    write_file(context, json.dumps(fields).encode())
    root = paths(args.home)[0]
    install_selected_binaries(args.binaries, root)
    write_file(root / "deploy/deploy.py", (HERE / "deploy.py").read_bytes())
    # Never auto-start an old release or restore old state after a failure.
    run(["systemctl", "--user", "start", "tl3-postgres.service"], timeout=150)
    if unit_state("tl3-postgres.service") != "active":
        raise RuntimeError("Postgres did not become active")
    wait_for_postgres(helper, config)
    require_stopped(*CLIENT_UNITS)
    run([root / "bin/tl3-deploy", "fresh-database", config, context], cwd=SOURCE, timeout=3400,
        input=capability + b"\n")
    run([root / "bin/tl3-deploy", "store-check", config], timeout=30)
    for unit in CLIENT_UNITS:
        run(["systemctl", "--user", "reset-failed", unit], timeout=15)
    run(["systemctl", "--user", "start", "tl3d.service"], timeout=150)
    deadline = time.monotonic() + 120
    address = tomllib.loads(config.read_text())["github"]["webhook_listen"]
    while True:
        if unit_state("tl3d.service") != "active":
            raise RuntimeError("selected tl3d failed startup")
        if listener_ready(address):
            break
        if time.monotonic() >= deadline:
            raise RuntimeError("tl3d listener did not become ready")
        time.sleep(1)
    run([root / "bin/tl3-deploy", "store-check", config], timeout=30)
    for unit in DEPENDENT_UNITS:
        run(["systemctl", "--user", "start", unit], timeout=150)
        if unit_state(unit) != "active":
            raise RuntimeError(f"{unit} failed startup")
    verify_stability(address)
    print("Fresh TL3 schema and listener ready; MLflow, status and console explicitly started")


def verify_stability(address):
    for _ in range(9):
        time.sleep(10)
        for unit in CLIENT_UNITS:
            restarts = run(["systemctl", "--user", "show", unit, "--property=NRestarts", "--value"], timeout=15)
            if unit_state(unit) != "active" or restarts.strip() != b"0":
                raise RuntimeError(f"{unit} failed bounded startup stability checks")
        if not listener_ready(address):
            raise RuntimeError("selected tl3d failed bounded startup stability checks")


def install_selected_binaries(binaries, root):
    selected = json.loads((binaries / "SOURCE.json").read_text())
    contents = {name: (binaries / name).read_bytes() for name in RELEASE_BINARIES}
    if selected.get("binaries") != {name: hashlib.sha256(data).hexdigest() for name, data in contents.items()}:
        raise RuntimeError("selected release binary hashes changed before install")
    for name, data in contents.items():
        write_file(root / "bin" / name, data, 0o700)
    write_file(root / "bin/SHA256SUMS", ''.join(
        f"{selected['binaries'][name]}  {name}\n" for name in RELEASE_BINARIES).encode())
    write_file(root / "bin/SOURCE.json", json.dumps(selected, sort_keys=True).encode())
    if check_binaries(root):
        raise RuntimeError("installed release binary verification failed")


def cold_cluster_identity(helper, backup):
    identifier = run([helper, "cold-cluster-identity", backup / "data"], timeout=30).decode().strip()
    if not re.fullmatch(r"[0-9]+", identifier):
        raise RuntimeError("cold backup has no verified Postgres system identifier")
    return identifier


def wait_for_postgres(helper, config):
    # Bound both retry count and each process, including authentication I/O.
    for attempt in range(3):
        try:
            run([helper, "database-ready", config], timeout=4)
            return
        except RuntimeError:
            if attempt == 2:
                raise RuntimeError("Postgres did not become ready; output suppressed") from None
            time.sleep(0.5)


def authorize_fresh_database(context, database_config, capability=b""):
    """Consume the current qualified orchestrator capability and recheck cold bytes."""
    private_file(context)
    fields = json.loads(context.read_text())
    capability = capability.removesuffix(b"\n")
    if (not re.fullmatch(rb"[0-9a-f]{64}", capability)
            or hashlib.sha256(capability).hexdigest() != fields.pop("capability_sha256", None)
            or not 0 < fields.pop("expires_monotonic", 0) - time.monotonic() <= 3600):
        raise RuntimeError("database reset needs the current one-use orchestrator capability")
    identifier = fields.pop("system_identifier", "")
    if not re.fullmatch(r"[0-9]+", identifier):
        raise RuntimeError("database reset lacks cold cluster identity")
    from argparse import Namespace
    args = Namespace(**fields)
    for name in ("home", "binaries", "state_manifest", "backup"):
        setattr(args, name, Path(getattr(args, name)))
    if context != args.backup / "reset-context.json":
        raise RuntimeError("database reset context is outside its diagnostic backup")

    sources, reset, config = fresh_scope(args)
    require_stopped(*CLIENT_UNITS)
    if unit_state("tl3-postgres.service") != "active":
        raise RuntimeError("database reset requires active, ready Postgres")
    verify_cold_backup(sources, args.backup, compare_sources=False)
    manifest = json.loads((args.backup / "manifest.json").read_text())
    if tree_manifest(sources["config"]) != manifest["trees"]["config"]:
        raise RuntimeError("configuration changed since the cold backup")
    for name in reset:
        root, child = name.split("/")
        if any((sources[root] / child).iterdir()):
            raise RuntimeError("file reset no longer matches the fresh-init boundary")
    if config != database_config:
        raise RuntimeError("database reset config scope mismatch")
    if cold_cluster_identity(args.binaries / "tl3-deploy", args.backup) != identifier:
        raise RuntimeError("cold cluster differs from the reset context identity")
    # Atomic consumption happens before any SQL mutation. A failed/drop-unknown
    # attempt needs a new orchestrated backup and token, never a context replay.
    consumed = context.with_suffix(".consumed")
    try:
        fd = os.open(consumed, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    except FileExistsError:
        raise RuntimeError("database reset capability already consumed") from None
    with os.fdopen(fd, "wb") as stream:
        stream.write(b"one-use authorization consumed\n")
        stream.flush()
        os.fsync(stream.fileno())
    sync_directory(consumed.parent)
    print(json.dumps({"system_identifier": identifier}))


def install(args):
    home = args.home
    root, config, _ = paths(home)
    private_dir(config)
    private_dir(root)
    postgres(home)
    checked_download(TOKENIZER_URL, TOKENIZER_SHA256, root / "tiktoken" / TOKENIZER_KEY)
    print("Tokenizer: pinned o200k_base installed and hash verified")
    install_release(home)
    private_dir(root / "bin")
    for binary in RELEASE_BINARIES:
        write_file(root / "bin" / binary, (args.binaries / binary).read_bytes(), 0o700)
    write_file(root / "bin/SHA256SUMS", (args.binaries / "SHA256SUMS").read_bytes())
    private_dir(root / "deploy")
    write_file(root / "deploy/deploy.py", (HERE / "deploy.py").read_bytes())
    state_path = root / "app-state.json"
    state = json.loads(state_path.read_text()) if state_path.exists() else {}
    destination = config / "tl3.toml"
    configure(home, args.status_listen, state, args.image, args.primary, args.alternate, args.console_listen)
    run([root / "bin/tl3-deploy", "parse", destination])
    grant_console_reader(home)
    units = home / ".config/systemd/user"
    units.mkdir(parents=True, exist_ok=True)
    for unit in [*(HERE / "units").glob("*.service"), *(HERE / "units").glob("*.slice")]:
        if unit.name != "tl3-cloudflared.service" or args.managed_tunnel:
            shutil.copy2(unit, units / unit.name)
    managed = root / "deploy/managed-tunnel"
    if args.managed_tunnel:
        write_file(managed, b"managed\n")
    else:
        managed.unlink(missing_ok=True)
    run(["systemctl", "--user", "daemon-reload"])
    collect_releases(home)
    print("Units installed; config parses. Run `deploy.py check` for Config::load and the credential checks.")


def release_id(source=SOURCE):
    """Name a worker release by its content, so redeploying the same code reuses it."""
    digest = hashlib.sha256()
    for package in RELEASE_PACKAGES:
        for path in sorted((source / package).rglob("*")):
            relative = path.relative_to(source)
            if path.is_dir() or any(fnmatch.fnmatch(part, pattern)
                                    for part in relative.parts for pattern in RELEASE_IGNORED):
                continue
            digest.update(str(relative).encode() + b"\0" + path.read_bytes() + b"\0")
    return digest.hexdigest()[:16]


def install_release(home, source=SOURCE):
    """Install the workers into an immutable release directory and switch `current` to it.

    Running workers keep the release they started from (#372), because the
    controller resolves `current` when it launches them. Returns whether this
    install introduced the selected immutable release."""
    root = paths(home)[0]
    releases = root / "releases"
    private_dir(releases)
    target = releases / release_id(source)
    if not (target / ".installed").exists():
        # Never rename a synced venv: it records its own absolute path.
        shutil.rmtree(target, ignore_errors=True)
        private_dir(target)
        for package in RELEASE_PACKAGES:
            shutil.copytree(source / package, target / package,
                            ignore=shutil.ignore_patterns(*RELEASE_IGNORED))
        run([home / ".local/bin/uv", "sync", "--locked", "--no-dev", "--project", target / "workers",
             "--python", "3.14+gil"])
        write_file(target / ".installed", b"installed\n")
        sync_directory(target)
    current = root / "current"
    introduced = not current.is_symlink()
    _link(root / "current", target)
    sync_directory(root)
    print(f"Workers: release {target.name} installed; locked Python 3.14 venv synced")
    return introduced


def _link(path, target):
    link = path.with_name(f"{path.name}.{os.getpid()}.tmp")
    link.unlink(missing_ok=True)
    link.symlink_to(target)
    os.replace(link, path)


def _alive(identity):
    """The worker record's process is still the one that wrote it (see process.rs)."""
    try:
        if (Path("/etc/machine-id").read_text().strip() != identity["machine"]
                or Path("/proc/sys/kernel/random/boot_id").read_text().strip() != identity["boot"]):
            return False
        fields = Path(f"/proc/{int(identity['pid'])}/stat").read_text().rsplit(")", 1)[1].split()
    except (OSError, KeyError, TypeError, ValueError, IndexError):
        return False
    return fields[0] not in ("Z", "X") and int(fields[19]) == identity["start"]


def live_pythons(runs):
    """Worker Python paths of attempts whose supervisor or worker still runs.

    None stands for a live attempt launched before #372, which ran from app/."""
    found = set()
    for attempt in runs.glob("*/attempt-*") if runs.is_dir() else ():
        identities = []
        for record in (attempt / "claim.json", attempt / "process.json"):
            try:
                identities.append(json.loads(record.read_text()))
            except (OSError, ValueError):
                continue
        if not any(isinstance(item, dict) and not item.get("cancelled") and _alive(item) for item in identities):
            continue
        try:
            found.add(json.loads((attempt / "launch.json").read_text())["python"])
        except FileNotFoundError:
            found.add(None)
        except (OSError, ValueError, KeyError):
            # An unreadable record could name any release; keep them all this time.
            found.add("*")
    return found


def collect_releases(home):
    """Keep the current release and every release a live run uses."""
    root, config, _ = paths(home)
    releases = root / "releases"
    try:
        runs = Path(tomllib.loads((config / "tl3.toml").read_text())["workers"]["runs"])
    except (OSError, KeyError, ValueError):
        runs = root / "worker-runs"
    live = live_pythons(runs)
    if "*" in live:
        print("Releases: a live attempt's launch record is unreadable; nothing collected")
        return
    keep = {(root / "current").resolve()}
    for python in live - {None}:
        path = Path(python)
        if path.is_relative_to(releases.resolve()):
            keep.add(releases.resolve() / path.relative_to(releases.resolve()).parts[0])
    for release in sorted(releases.iterdir()) if releases.is_dir() else ():
        if release.resolve() not in keep and not release.name.startswith("."):
            shutil.rmtree(release)
            print(f"Releases: removed {release.name}")


def _tl3d_state():
    """tl3d.service's ActiveState: active, activating, reloading, deactivating, inactive or failed."""
    result = subprocess.run(["systemctl", "--user", "show", "tl3d.service", "-p", "ActiveState", "--value"],
                            stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True)
    return result.stdout.strip() or "unknown"


def _kill_cgroup(scope):
    (scope / "cgroup.kill").write_text("1")


PROBE_TIMEOUT = 20


def launcher_works(home):
    """Start a throwaway run through the configured launcher and stop it as tl3d does (#372, #394).

    The probe must land in a scope of its own, outside this cgroup, and
    cgroup.kill must end it. Returns whether it worked and a one-line reason."""
    bus = Path(os.environ.get("XDG_RUNTIME_DIR", f"/run/user/{os.getuid()}")) / "bus"
    if not bus.is_socket():
        return False, f"no user bus at {bus}"
    try:
        launcher = tomllib.loads((paths(home)[1] / "tl3.toml").read_text())["workers"].get("launcher") or []
    except (OSError, KeyError, ValueError) as error:
        return False, f"tl3.toml is unreadable ({type(error).__name__})"
    if not launcher:
        return False, "tl3.toml sets no workers.launcher, so runs would stay in tl3d's cgroup"
    own = Path("/proc/self/cgroup").read_text()
    with tempfile.TemporaryFile() as errors:
        try:
            # Print the cgroup the command lands in, then wait to be killed.
            probe = subprocess.Popen([*launcher, "/bin/sh", "-c", f"cat /proc/self/cgroup; exec sleep {2 * PROBE_TIMEOUT}"],
                                     stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=errors)
        except OSError as error:
            return False, f"{launcher[0]}: {error.strerror}"

        def failed(reason):
            if probe.poll() is None:
                probe.kill()
            probe.wait()
            errors.seek(0)
            output = errors.read(2048).decode(errors="replace").strip()
            return False, reason + (f": {output.splitlines()[-1]}" if output else "")

        line = b""
        while not line.startswith(b"0::"):
            if not select.select([probe.stdout], [], [], PROBE_TIMEOUT)[0]:
                return failed(f"the launcher timed out after {PROBE_TIMEOUT} s")
            line = probe.stdout.readline()
            if not line:
                return failed(f"the launcher exited {probe.wait()} before the probe ran")
        cgroup = line.decode().strip()[3:]
        if not cgroup.endswith(".scope") or f"0::{cgroup}" in own.splitlines():
            return failed(f"the probe ran in {cgroup}, not a scope of its own")
        scope = Path("/sys/fs/cgroup") / cgroup.lstrip("/")
        try:
            _kill_cgroup(scope)
        except OSError as error:
            return failed(f"cgroup.kill on {scope.name} failed ({error.strerror})")
        try:
            code = probe.wait(timeout=PROBE_TIMEOUT)
        except subprocess.TimeoutExpired:
            return failed(f"cgroup.kill did not stop {scope.name}")
        if code != -signal.SIGKILL:
            return failed(f"the probe in {scope.name} ended with {code}, not by cgroup.kill")
        return True, f"{scope.name} ran outside tl3d's cgroup and cgroup.kill stopped it"


def stop_runs(home):
    """Stop every worker run. A tl3d stop or restart no longer does (#372)."""
    state = _tl3d_state()
    if state not in ("inactive", "failed"):
        raise RuntimeError(f"Stop tl3d first (systemctl --user stop tl3d.service; it is {state}). A running "
                           "tl3d starts a new attempt for each stopped run once it proves the run is gone.")
    run(["systemctl", "--user", "stop", WORKER_SLICE])
    print("Stopped every TL3 worker run. When tl3d starts, each counts as a failed run under the "
          "usual retry rules.")


def grant_console_reader(home):
    """Create or update the console's login: it reads, files commands and edits prompts.

    Postgres also does this each time it starts."""
    root, config, _ = paths(home)
    active = subprocess.run(["systemctl", "--user", "is-active", "--quiet", "tl3-postgres.service"]).returncode == 0
    if active:
        run([root / "bin/tl3-deploy", "console-role", config / "tl3.toml"])
        print("Console: Postgres login granted (reads, files commands, edits prompts)")
    else:
        print("Console: the Postgres login is granted when tl3-postgres starts")


def service(home, name):
    root, config, _ = paths(home)
    runtime = Path(os.environ["RUNTIME_DIRECTORY"])
    credentials = Path(os.environ["CREDENTIALS_DIRECTORY"])
    text = (config / "tl3.toml").read_text()
    cache = tomllib.loads(text)["workers"].get("tiktoken_cache_dir")
    if name == "controller" and cache:
        os.environ["TIKTOKEN_CACHE_DIR"] = cache
    # RobTand/tl3-console runs the console from its own unit and config (PRD #327).
    if name not in ("database", "status", "controller"):
        raise RuntimeError(f"unknown service {name}")
    # The database service reads the console's credential in place, so a missing
    # file can't fail tl3-postgres.
    for source, filename in ((config / "secrets/postgres-password", "postgres-password"),
                             (config / "github-app.json", "github-app.json"),
                             (config / "secrets/model-key", "model-key"),
                             (config / "secrets/hindsight-key", "hindsight-key")):
        text = text.replace(json.dumps(str(source)), json.dumps(str(credentials / filename)))
    target = runtime / "tl3.toml"
    write_file(target, text.encode())
    if name == "database":
        os.execv(root / "bin/tl3-deploy", ["tl3-deploy", "database", str(target)])
    elif name == "status":
        os.execv(root / "bin/tl3", ["tl3", "status-serve", "--config", str(target)])
    else:
        os.execv(root / "bin/tl3d", ["tl3d", "--config", str(target)])


def start(home):
    run(["systemctl", "--user", "enable", "--now", "tl3-postgres.service"])
    run(["systemctl", "--user", "enable", "--now", "tl3-status.service"])
    run(["systemctl", "--user", "enable", "--now", "tl3d.service"])
    print("Postgres and status enabled. tl3d is gated until production startup and model setup are ready.")


def mlflow_health(home):
    try:
        data = tomllib.loads((home / ".config/tl3/mlflow.toml").read_text())["mlflow"]
        from urllib.parse import urlsplit
        address = urlsplit(data["tracking_uri"])
        connection = http.client.HTTPConnection(address.hostname, address.port, timeout=3)
        try:
            connection.request("GET", "/health")
            ok = connection.getresponse().status == 200
        finally:
            connection.close()
    except (OSError, KeyError, ValueError):
        ok = False
    print(f"{'OK' if ok else 'MISSING'} MLflow HTTP")
    return ok


def install_mlflow(home):
    root, config, _ = paths(home)
    private_dir(root / "mlflow")
    private_dir(root / "mlflow/artifacts")
    for name in ("pyproject.toml", "uv.lock"):
        write_file(root / "mlflow" / name, (HERE / "mlflow" / name).read_bytes())
    run([home / ".local/bin/uv", "sync", "--locked", "--no-dev", "--project", root / "mlflow"])
    write_file(root / "deploy/mlflow-server.py", (HERE / "mlflow-server.py").read_bytes())
    destination = config / "mlflow.toml"
    if not destination.exists():
        write_file(destination, b'[mlflow]\ntracking_uri = "http://192.168.1.107:5033"\nexperiment = "tl3"\n')
    unit_dir = home / ".config/systemd/user"
    unit_dir.mkdir(parents=True, exist_ok=True)
    shutil.copy2(HERE / "units/tl3-mlflow.service", unit_dir / "tl3-mlflow.service")
    write_file(unit_dir / "tl3d.service.d/mlflow.conf",
               b'[Service]\nEnvironment=TL3_MLFLOW_CONFIG=%h/.config/tl3/mlflow.toml\n')
    run(["systemctl", "--user", "daemon-reload"])
    print("MLflow 3.17.2 installed. Controller tracing takes effect on its next start.")


def check_binaries(root):
    """Verify exactly TL3's release binaries against the installed release SHA256SUMS.

    Returns the names that failed. Other files in bin/ belong to other deployers
    (tl3-console is installed there by RobTand/tl3-console's own deploy) and are
    neither verified nor flagged.
    """
    directory = root / "bin"
    expected = {}
    try:
        for line in (directory / "SHA256SUMS").read_text().splitlines():
            entry = re.fullmatch(r"([0-9a-f]{64}) [ *]([A-Za-z0-9][A-Za-z0-9._-]*)", line)
            if not entry or entry[2] not in RELEASE_BINARIES or entry[2] in expected:
                print("MISSING bin/SHA256SUMS: invalid release manifest")
                return set(RELEASE_BINARIES)
            expected[entry[2]] = entry[1]
    except (OSError, UnicodeError):
        print("MISSING bin/SHA256SUMS: unreadable release manifest")
        return set(RELEASE_BINARIES)
    failed = set()
    for name in RELEASE_BINARIES:
        path = directory / name
        ok = False
        if name in expected and path.is_file():
            try:
                with path.open("rb") as binary:
                    ok = hashlib.file_digest(binary, "sha256").hexdigest() == expected[name]
            except OSError:
                pass
        print(f"{'OK' if ok else 'MISSING'} bin/{name}: release SHA-256")
        if not ok:
            failed.add(name)
    return failed


def check(home):
    root, config, _ = paths(home)
    failed_binaries = check_binaries(root)
    missing = len(failed_binaries) + int(not mlflow_health(home))
    managed = (root / "deploy/managed-tunnel").exists()
    units = ["tl3-postgres", "tl3d", "tl3-status", "tl3-mlflow"]
    if managed:
        units.append("tl3-cloudflared")
    else:
        print("OK external webhook route selected; verify public reception with a signed ping")
    for unit in units:
        result = subprocess.run(["systemctl", "--user", "is-active", unit], capture_output=True)
        active = result.returncode == 0
        print(f"{'OK' if active else 'MISSING'} {unit}: {'active' if active else 'inactive or failed'}")
        missing += not active
    for label, command in (
        ("Postgres reachable and migrated", [root / "bin/tl3-deploy", "store-check", config / "tl3.toml"]),
        ("tl3 status", [root / "bin/tl3", "status", "--config", config / "tl3.toml"]),
        ("Config::load including secret metadata", [root / "bin/tl3-deploy", "validate", config / "tl3.toml"]),
    ):
        if Path(command[0]).name in failed_binaries:
            print(f"MISSING {label}: binary verification failed")
            missing += 1
            continue
        result = subprocess.run([str(part) for part in command], capture_output=True)
        print(f"{'OK' if result.returncode == 0 else 'MISSING'} {label}")
        missing += result.returncode != 0
    data = tomllib.loads((config / "tl3.toml").read_text())
    for label, ok in (
        ("operations issue configured", data.get("operations", {}).get("issue", 0) > 0),
        ("model names configured", all(step["name"] not in {"default-model", "alternate-model"}
                                       for step in data["models"]["maker"])),
        ("sandbox installation configured", data["github"]["repos"][0]["installation_id"] != 1),
        ("harness image configured", data["prismabuild"]["container_image"] != "sha256:" + "0" * 64),
    ):
        print(f"{'OK' if ok else 'MISSING'} {label}")
        missing += not ok
    for label, address, path, expected in (
        ("LAN status HTTP", data["status"]["listen"], "/", 200),
        ("loopback webhook HTTP", data["github"]["webhook_listen"], "/github/events", 405),
    ):
        if not address:
            print(f"MISSING {label}")
            missing += 1
            continue
        host, port = address.rsplit(":", 1)
        connection = http.client.HTTPConnection(host, int(port), timeout=3)
        try:
            connection.request("GET", path)
            ok = connection.getresponse().status == expected
        except OSError:
            ok = False
        finally:
            connection.close()
        print(f"{'OK' if ok else 'MISSING'} {label}")
        missing += not ok
    credentials = [("model credential", config / "secrets/model-key"),
                   ("Hindsight credential", config / "secrets/hindsight-key")]
    if managed:
        credentials.append(("Cloudflare token", config / "secrets/cloudflared-token"))
    for label, path in credentials:
        try:
            private_file(path)
            ok = True
        except (OSError, RuntimeError):
            ok = False
        print(f"{'OK' if ok else 'MISSING'} {label}")
        missing += not ok
    ok, detail = launcher_works(home)
    print(f"{'OK' if ok else 'MISSING'} worker launcher: {detail}")
    missing += not ok
    ready = (config / "production-ready").exists()
    print(f"{'OK' if ready else 'MISSING'} production startup marker after #20")
    missing += not ready
    print(f"Deployment check: {missing} missing checks")
    return 1 if missing else 0


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=["install", "initdb", "start", "check", "service", "model-defaults", "install-mlflow",
                                            "stop-runs", "fresh-init", "qualify-fresh", "authorize-fresh-database"])
    parser.add_argument("--home", type=Path, default=Path.home())
    parser.add_argument("--binaries", type=Path, default=SOURCE / "target/release")
    parser.add_argument("--status-listen", default="192.168.1.107:8033")
    parser.add_argument("--console-listen", help="default: the status address on port 8034")
    parser.add_argument("--managed-tunnel", action="store_true", help="install and require TL3's own Cloudflare connector; default uses the external route")
    parser.add_argument("--image")
    parser.add_argument("--primary")
    parser.add_argument("--alternate")
    parser.add_argument("--service", choices=["database", "status", "controller"])
    parser.add_argument("--head", help="full chosen deployment commit")
    parser.add_argument("--lane", choices=["release", "dev"], default="release")
    parser.add_argument("--state-manifest", type=Path, help="private exact owned-state scope manifest")
    parser.add_argument("--backup", type=Path, help="new absolute private cold-backup destination")
    parser.add_argument("--context", type=Path)
    parser.add_argument("--database-config", type=Path)
    args = parser.parse_args()
    if args.command == "authorize-fresh-database":
        if not args.context or not args.database_config:
            parser.error("database authorization requires --context and --database-config")
        authorize_fresh_database(args.context, args.database_config, sys.stdin.buffer.read(65))
        return 0
    if args.command == "fresh-init":
        if not args.state_manifest or not args.backup or not args.backup.is_absolute():
            parser.error("fresh-init requires --state-manifest and absolute --backup")
        fresh_init(args)
        return 0
    if args.command == "qualify-fresh":
        qualify_fresh(args)
        return 0
    if args.command == "check":
        return check(args.home)
    if args.command == "service":
        service(args.home, args.service)
    elif args.command == "install-mlflow":
        install_mlflow(args.home)
    elif args.command == "model-defaults":
        model_defaults(args.home)
    elif args.command == "install":
        install(args)
    elif args.command == "stop-runs":
        stop_runs(args.home)
    else:
        globals()[args.command](args.home)
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except Exception as error:
        # Upstream errors and tracebacks may contain sensitive inputs.
        if isinstance(error, RuntimeError):
            print(str(error), file=sys.stderr)
        else:
            print(f"Deployment failed ({type(error).__name__}); details suppressed", file=sys.stderr)
        sys.exit(1)
