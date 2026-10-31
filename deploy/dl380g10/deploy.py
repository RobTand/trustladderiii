#!/usr/bin/env python3
"""Install and inspect TL3 on dl380g10 without printing credentials."""
import argparse
from datetime import datetime, timezone
import hashlib
import http.client
import io
import json
import os
from pathlib import Path
import re
import secrets
import shutil
import socket
import stat
import subprocess
import sys
import tarfile
import tempfile
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
TOKENIZER_URL = "https://openaipublic.blob.core.windows.net/encodings/o200k_base.tiktoken"
TOKENIZER_SHA256 = "446a9538cb6c348e3516120d7c08b09f57c36495e2acfffe59a5bf8b0cfb1a2d"
TOKENIZER_KEY = hashlib.sha1(TOKENIZER_URL.encode()).hexdigest()


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
            os.rename(directory / "unpacked", binaries)
    if not (binaries / "bin/postgres").is_file():
        raise RuntimeError("incomplete versioned Postgres installation")
    credential = config / "secrets/postgres-password"
    if (root / "postgres/data").exists() and not credential.exists():
        raise RuntimeError("existing Postgres data needs its original password file; refusing to generate another")
    password(credential)
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
            raise RuntimeError("existing cluster has another major version; migrate explicitly")
        return
    with tempfile.TemporaryDirectory(prefix="initdb-", dir=data.parent) as directory:
        stage = Path(directory) / "data"
        run([binaries / "bin/initdb", "-D", stage, "-U", "tl3", "--encoding=UTF8",
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


def render_config(home, listen, installation=1, issue=None, image=None, primary=None, alternate=None):
    address, port = listen.rsplit(":", 1)
    socket.inet_pton(socket.AF_INET, address)
    if address.startswith("127.") or address == "0.0.0.0" or not 0 < int(port) < 65536:
        raise RuntimeError("status must use the LAN interface address and a fixed port")
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
        '"/srv/tl3/workers/.venv/bin/python"': json.dumps(str(root / "app/workers/.venv/bin/python")),
        '"/srv/tl3/worker-runs"': json.dumps(str(root / "worker-runs")),
        '# tiktoken_cache_dir = "/srv/tl3/tiktoken"': "tiktoken_cache_dir = " + json.dumps(str(root / "tiktoken")),
    }
    if image:
        replacements['sha256:' + '0' * 64] = image
    if primary:
        replacements['gpt-6.1-sol'] = primary
    if alternate:
        replacements['claude-fable-5-1'] = alternate
    for old, new in replacements.items():
        text = text.replace(old, new)
    text = text.replace('[github]\n', '[github]\nwebhook_listen = "127.0.0.1:8081"\n')
    if issue is not None:
        text = text.replace('[github]\n', '[github]\noperations = ' + toml_value({"owner": "RobTand", "name": "trustladderiii", "number": issue, "installation_id": installation}) + '\n')
        text += f'\n[operations]\nowner = "RobTand"\nrepo = "trustladderiii"\nissue = {issue}\n'
    tomllib.loads(text)
    return text


def add_fields(text, table, fields):
    """Insert missing fields without reserializing operator settings or comments."""
    header = re.search(r"(?m)^" + re.escape(table) + r"\s*(?:#.*)?$", text)
    additions = "".join(f"{key} = {value}\n" for key, value in fields.items())
    if header:
        return text[:header.end()] + "\n" + additions + text[header.end():]
    return text + "\n" + table + "\n" + additions


def configure(home, listen, state, image=None, primary=None, alternate=None):
    _, config, _ = paths(home)
    destination = config / "tl3.toml"
    defaults = render_config(home, listen, state.get("installation_id", 1),
                             state.get("operations_issue"), image, primary, alternate)
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
    text = original
    # Deployment migrations fill missing fields. Existing values always win.
    for table, fields in (
        ("github", {"webhook_listen": "127.0.0.1:8081"}),
        ("workers", {"tiktoken_cache_dir": str(paths(home)[0] / "tiktoken")}),
        ("hindsight", {"bank": tomllib.loads(defaults)["hindsight"]["bank"]}),
        ("models", {"writer": tomllib.loads(defaults)["models"]["writer"]}),
        ("hindsight", {"api_key_file": str(config / "secrets/hindsight-key")}),
    ):
        missing = {key: toml_value(value) for key, value in fields.items()
                   if key not in data.get(table, {})}
        if missing:
            text = add_fields(text, f"[{table}]", missing)
    if "installation_id" not in data["github"]["repos"][0]:
        text = add_fields(text, "[[github.repos]]", {"installation_id": state.get("installation_id", 1)})
    if "operations_issue" in state:
        operations = {"owner": "RobTand", "repo": "trustladderiii", "issue": state["operations_issue"]}
        missing = {key: toml_value(value) for key, value in operations.items()
                   if key not in data.get("operations", {})}
        if missing:
            text = add_fields(text, "[operations]", missing)
    if "operations" not in data["github"] and data.get("operations"):
        operations = data["operations"]
        identity = {"owner": operations["owner"], "name": operations["repo"],
                    "number": operations["issue"], "installation_id": data["github"]["repos"][0].get("installation_id", state.get("installation_id", 1))}
        text = add_fields(text, "[github]", {"operations": toml_value(identity)})
    tomllib.loads(text)
    save_config(destination, original, text)


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
    result = subprocess.run([str(part) for part in command], stdout=subprocess.PIPE,
                            stderr=subprocess.PIPE, **kwargs)
    if result.returncode:
        raise RuntimeError(f"{Path(command[0]).name} failed with exit {result.returncode}; output suppressed")
    return result.stdout


def install(args):
    home = args.home
    root, config, _ = paths(home)
    private_dir(config)
    private_dir(root)
    postgres(home)
    checked_download(TOKENIZER_URL, TOKENIZER_SHA256, root / "tiktoken" / TOKENIZER_KEY)
    print("Tokenizer: pinned o200k_base installed and hash verified")
    app = root / "app"
    private_dir(app)
    for package in ("workers", "harness"):
        shutil.copytree(SOURCE / package, app / package, dirs_exist_ok=True,
                        ignore=shutil.ignore_patterns(".venv", "__pycache__", ".pytest_cache", "*.egg-info"))
    uv = home / ".local/bin/uv"
    run([uv, "sync", "--locked", "--no-dev", "--project", app / "workers", "--python", "3.14+gil"])
    print("Workers: locked Python 3.14 venv synced; launches use Python directly")
    private_dir(root / "bin")
    for binary in ("tl3", "tl3d", "tl3-deploy"):
        write_file(root / "bin" / binary, (args.binaries / binary).read_bytes(), 0o700)
    private_dir(root / "deploy")
    write_file(root / "deploy/deploy.py", (HERE / "deploy.py").read_bytes())
    state_path = root / "app-state.json"
    state = json.loads(state_path.read_text()) if state_path.exists() else {}
    destination = config / "tl3.toml"
    configure(home, args.status_listen, state, args.image, args.primary, args.alternate)
    run([root / "bin/tl3-deploy", "parse", destination])
    units = home / ".config/systemd/user"
    units.mkdir(parents=True, exist_ok=True)
    for unit in (HERE / "units").glob("*.service"):
        if unit.name != "tl3-cloudflared.service" or args.managed_tunnel:
            shutil.copy2(unit, units / unit.name)
    managed = root / "deploy/managed-tunnel"
    if args.managed_tunnel:
        write_file(managed, b"managed\n")
    else:
        managed.unlink(missing_ok=True)
    run(["systemctl", "--user", "daemon-reload"])
    print("Units installed; config parses. Run `deploy.py check` for Config::load and the credential checks.")


def service(home, name):
    root, config, _ = paths(home)
    runtime = Path(os.environ["RUNTIME_DIRECTORY"])
    credentials = Path(os.environ["CREDENTIALS_DIRECTORY"])
    text = (config / "tl3.toml").read_text()
    cache = tomllib.loads(text)["workers"].get("tiktoken_cache_dir")
    if name == "controller" and cache:
        os.environ["TIKTOKEN_CACHE_DIR"] = cache
    for source, filename in ((config / "secrets/postgres-password", "postgres-password"),
                             (config / "github-app.json", "github-app.json"),
                             (config / "secrets/model-key", "model-key"),
                             (config / "secrets/hindsight-key", "hindsight-key")):
        text = text.replace(json.dumps(str(source)), json.dumps(str(credentials / filename)))
    target = runtime / "tl3.toml"
    write_file(target, text.encode())
    if name == "database":
        os.execv(root / "bin/tl3-deploy", ["tl3-deploy", "database", str(target)])
    if name == "status":
        os.execv(root / "bin/tl3", ["tl3", "status-serve", "--config", str(target)])
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


def check(home):
    root, config, _ = paths(home)
    missing = int(not mlflow_health(home))
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
    ready = (config / "production-ready").exists()
    print(f"{'OK' if ready else 'MISSING'} production startup marker after #20")
    missing += not ready
    print(f"Deployment check: {missing} missing checks")
    return 1 if missing else 0


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=["install", "initdb", "start", "check", "service", "model-defaults", "install-mlflow"])
    parser.add_argument("--home", type=Path, default=Path.home())
    parser.add_argument("--binaries", type=Path, default=SOURCE / "target/release")
    parser.add_argument("--status-listen", default="192.168.1.107:8033")
    parser.add_argument("--managed-tunnel", action="store_true", help="install and require TL3's own Cloudflare connector; default uses the external route")
    parser.add_argument("--image")
    parser.add_argument("--primary")
    parser.add_argument("--alternate")
    parser.add_argument("--service", choices=["database", "status", "controller"])
    args = parser.parse_args()
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
