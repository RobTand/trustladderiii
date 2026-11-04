import importlib.util
import io
import contextlib
import hashlib
import json
import os
from pathlib import Path
import socket
import stat
import subprocess
import tempfile
import time
import tomllib
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("deployment", Path(__file__).with_name("deploy.py"))
deploy = importlib.util.module_from_spec(spec)
spec.loader.exec_module(deploy)


class DeployTests(unittest.TestCase):
    def setUp(self):
        self.scratch = tempfile.TemporaryDirectory(prefix="tl3-deploy-test-")
        self.addCleanup(self.scratch.cleanup)
        self.home = Path(self.scratch.name)
        self.helper = Path(os.environ["TL3_DEPLOY_HELPER"])

    def config(self):
        text = deploy.render_config(self.home, "192.168.1.107:8033", 987, 123,
                                    "sha256:" + "a" * 64, "gpt-test", "claude-test")
        config = self.home / "tl3.toml"
        config.write_text(text)
        return config, tomllib.loads(text)

    def test_generated_config_loads_and_status_needs_no_model(self):
        config, data = self.config()
        self.assertEqual(data["github"]["repos"], [{"owner": "RobTand", "name": "tl3-sandbox", "installation_id": 987}])
        self.assertEqual(data["operations"]["issue"], 123)
        self.assertEqual(data["store"]["host"], str(self.home / ".local/share/tl3/postgres/socket"))
        self.assertNotIn("default-model", config.read_text())
        for path in (data["github"]["app_credentials_file"], data["models"]["key_file"], data["store"]["password_file"]):
            deploy.write_file(Path(path), b"fixture only\n")
        result = subprocess.run([self.helper, "validate", config], capture_output=True)
        self.assertEqual(result.returncode, 0, result.stderr.decode())
        Path(data["models"]["key_file"]).unlink()
        result = subprocess.run([self.helper, "validate", config], capture_output=True)
        self.assertNotEqual(result.returncode, 0)

    def test_operations_rejects_zero(self):
        config, _ = self.config()
        config.write_text(config.read_text().replace("issue = 123", "issue = 0"))
        result = subprocess.run([self.helper, "parse", config], capture_output=True)
        self.assertNotEqual(result.returncode, 0)

    def test_lan_binding_rejects_wildcard_and_loopback(self):
        for address in ("0.0.0.0:8033", "127.0.0.1:8033", "192.168.1.107:0"):
            with self.assertRaises(RuntimeError):
                deploy.render_config(self.home, address)

    def test_password_rerun_never_reads_or_replaces_existing_secret(self):
        secret = self.home / "password"
        deploy.password(secret)
        info = secret.stat()
        self.assertEqual(stat.S_IMODE(info.st_mode), 0o600)
        with patch.object(Path, "read_text", side_effect=AssertionError("secret read")), \
             patch.object(Path, "read_bytes", side_effect=AssertionError("secret read")):
            deploy.password(secret)
        self.assertEqual(info.st_ino, secret.stat().st_ino)
        self.assertEqual(info.st_mtime_ns, secret.stat().st_mtime_ns)
        secret.chmod(0o644)
        with self.assertRaises(RuntimeError):
            deploy.password(secret)

    def test_download_verifies_hash_before_publication(self):
        destination = self.home / "cache/data"
        with patch.object(deploy.urllib.request, "urlopen", return_value=io.BytesIO(b"damaged")):
            with self.assertRaises(RuntimeError):
                deploy.checked_download("https://example.invalid", "0" * 64, destination)
        self.assertFalse(destination.exists())
        content = b"pinned fixture"
        with patch.object(deploy.urllib.request, "urlopen", return_value=io.BytesIO(content)):
            deploy.checked_download("https://example.invalid", hashlib.sha256(content).hexdigest(), destination)
        self.assertEqual(destination.read_bytes(), content)
        with patch.object(deploy.urllib.request, "urlopen", side_effect=AssertionError("unnecessary fetch")):
            deploy.checked_download("https://example.invalid", hashlib.sha256(content).hexdigest(), destination)

    def test_existing_data_never_gets_a_replacement_password(self):
        root, config, binaries = deploy.paths(self.home)
        (binaries / "bin").mkdir(parents=True)
        (binaries / "bin/postgres").touch()
        (root / "postgres/data").mkdir(parents=True)
        with self.assertRaisesRegex(RuntimeError, "original password"):
            deploy.postgres(self.home)
        self.assertFalse((config / "secrets/postgres-password").exists())

    def test_service_uses_systemd_credentials_and_direct_python(self):
        config, data = self.config()
        deployed = self.home / ".config/tl3/tl3.toml"
        deploy.write_file(deployed, config.read_bytes())
        runtime = self.home / "runtime"
        credentials = self.home / "credentials"
        with patch.dict(os.environ, {"RUNTIME_DIRECTORY": str(runtime), "CREDENTIALS_DIRECTORY": str(credentials)}), \
             patch.object(os, "execv") as execute:
            deploy.service(self.home, "controller")
            self.assertEqual(os.environ["TIKTOKEN_CACHE_DIR"], data["workers"]["tiktoken_cache_dir"])
        runtime_config = tomllib.loads((runtime / "tl3.toml").read_text())
        self.assertEqual(runtime_config["hindsight"].get("api_key_file"), str(credentials / "hindsight-key"))
        self.assertIn("LoadCredential=hindsight-key:%h/.config/tl3/secrets/hindsight-key",
                      (deploy.HERE / "units/tl3d.service").read_text())
        self.assertEqual(runtime_config["models"]["key_file"], str(credentials / "model-key"))
        self.assertEqual(runtime_config["github"]["app_credentials_file"], str(credentials / "github-app.json"))
        self.assertEqual(execute.call_args.args[1][-2:], ["--config", str(runtime / "tl3.toml")])
        self.assertNotIn("uv", runtime_config["workers"]["python"])

    def test_unit_syntax_and_ordering(self):
        units = self.home / "units"
        units.mkdir()
        for source in [*(deploy.HERE / "units").glob("*.service"), *(deploy.HERE / "units").glob("*.slice")]:
            lines = []
            for line in source.read_text().replace("%h", str(self.home)).splitlines():
                key, _, command = line.partition("=")
                if key in ("ExecStart", "ExecStartPre", "ExecStartPost", "ExecStop"):
                    executable = Path(command.split()[0])
                    if not executable.is_relative_to(self.home):
                        # CI runners lack host binaries such as cloudflared, so every
                        # command runs a stub and verify checks syntax and ordering.
                        stub = self.home / "host" / executable.name
                        line = f"{key}={stub}{command[len(str(executable)):]}"
                        executable = stub
                    deploy.write_file(executable, b"#!/bin/sh\nexit 0\n", 0o700)
                lines.append(line)
            (units / source.name).write_text("\n".join(lines) + "\n")
        runtime = self.home / "runtime"
        runtime.mkdir(mode=0o700)
        environment = os.environ | {"XDG_RUNTIME_DIR": str(runtime)}
        result = subprocess.run(["systemd-analyze", "--user", "--man=no", "verify", *map(str, units.glob("*.service")),
                                 *map(str, units.glob("*.slice"))], capture_output=True, env=environment)
        self.assertEqual(result.returncode, 0, result.stderr.decode())
        controller = (units / "tl3d.service").read_text()
        self.assertIn("KillMode=control-group", controller)
        self.assertIn("Requires=tl3-postgres.service", controller)
        self.assertIn("After=tl3-postgres.service", controller.splitlines())
        self.assertIn(f"ConditionPathExists={self.home}/.config/tl3/production-ready", controller.splitlines())
        self.assertIn("LoadCredential=model-key:", controller)
        cloudflared = (units / "tl3-cloudflared.service").read_text()
        self.assertIn("--token-file %d/token", cloudflared)

    def test_private_credentials_require_nonempty_owner_only_regular_files(self):
        credential = self.home / "credential"
        with self.assertRaises(OSError):
            deploy.private_file(credential)
        deploy.write_file(credential, b"")
        with self.assertRaises(RuntimeError):
            deploy.private_file(credential)
        deploy.write_file(credential, b"fixture")
        deploy.private_file(credential)
        credential.chmod(0o644)
        with self.assertRaises(RuntimeError):
            deploy.private_file(credential)
        credential.chmod(0o600)
        link = self.home / "link"
        link.symlink_to(credential)
        with self.assertRaises(RuntimeError):
            deploy.private_file(link)
        with self.assertRaises(RuntimeError):
            deploy.private_file(self.home)

    def test_config_migration_preserves_operator_settings_and_backs_up_once(self):
        config, _ = self.config()
        original = config.read_text().replace('installation_id = 987\n', '')
        original = original[:original.index('\n[operations]')]
        original = original.replace('budget_tokens = 128000', 'budget_tokens = 120000')
        original = original.replace('effort = "max"', 'effort = "xhigh"')
        destination = self.home / ".config/tl3/tl3.toml"
        deploy.write_file(destination, original.encode())
        deploy.configure(self.home, "192.168.1.107:8033", {"installation_id": 987, "operations_issue": 123},
                         "sha256:" + "b" * 64, "replacement", "replacement")
        actual = tomllib.loads(destination.read_text())
        before = tomllib.loads(original)
        self.assertEqual(actual["models"], before["models"])
        self.assertEqual(actual["prismabuild"], before["prismabuild"])
        self.assertEqual(actual["github"]["repos"][0]["installation_id"], 987)
        self.assertEqual(actual["operations"]["issue"], 123)
        backups = list(destination.parent.glob("tl3.toml.*.bak"))
        self.assertEqual(len(backups), 1)
        self.assertEqual(backups[0].read_text(), original)
        self.assertEqual(stat.S_IMODE(backups[0].stat().st_mode), 0o600)
        info = destination.stat()
        deploy.configure(self.home, "192.168.1.107:8033", {"installation_id": 999, "operations_issue": 999})
        self.assertEqual(destination.stat().st_mtime_ns, info.st_mtime_ns)
        self.assertEqual(list(destination.parent.glob("tl3.toml.*.bak")), backups)

    def test_configure_says_which_flags_an_existing_config_ignores(self):
        config, _ = self.config()
        destination = self.home / ".config/tl3/tl3.toml"
        deploy.write_file(destination, config.read_bytes())
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            deploy.configure(self.home, "192.168.1.107:8033", {"installation_id": 987, "operations_issue": 123},
                             "sha256:" + "b" * 64, "replacement")
        self.assertIn("--image", output.getvalue())
        self.assertIn("--primary", output.getvalue())
        self.assertNotIn("--alternate", output.getvalue())
        self.assertEqual(destination.read_bytes(), config.read_bytes())

    def test_older_hindsight_config_migrates_without_replacing_endpoint_or_bank(self):
        config, _ = self.config()
        destination = self.home / ".config/tl3/tl3.toml"
        for bank in (None, "operator-bank"):
            with self.subTest(bank=bank):
                original = config.read_text().replace('bank = "tl3"\n', '')
                if bank:
                    original = original.replace('[hindsight]\n', f'[hindsight]\nbank = "{bank}"\n')
                before = tomllib.loads(original)["hindsight"]
                deploy.write_file(destination, original.encode())
                deploy.configure(self.home, "192.168.1.107:8033", {})
                parsed = subprocess.run([self.helper, "parse", destination], capture_output=True)
                self.assertEqual(parsed.returncode, 0, parsed.stderr.decode())
                actual = tomllib.loads(destination.read_text())["hindsight"]
                self.assertEqual(actual["endpoint"], before["endpoint"])
                self.assertEqual(actual["bank"], bank or "tl3")

    def test_configure_explains_a_config_without_github_repos(self):
        destination = self.home / ".config/tl3/tl3.toml"
        deploy.write_file(destination, b"[store]\nhost = \"x\"\n")
        with self.assertRaisesRegex(SystemExit, r"\[\[github\.repos\]\]"):
            deploy.configure(self.home, "192.168.1.107:8033", {"installation_id": 987})

    def test_model_defaults_are_explicit_and_preserve_endpoint_and_key_path(self):
        config, data = self.config()
        destination = self.home / ".config/tl3/tl3.toml"
        deploy.write_file(destination, config.read_bytes())
        deploy.model_defaults(self.home)
        actual = tomllib.loads(destination.read_text())["models"]
        self.assertEqual(actual["base_url"], data["models"]["base_url"])
        self.assertEqual(actual["key_file"], data["models"]["key_file"])
        defaults = tomllib.loads((deploy.HERE / "models.toml").read_text())["models"]
        for role, ladder in defaults.items():
            self.assertEqual(actual[role], ladder)
        # #416: attempt 2 escalates to Opus 5.5 at high effort.
        self.assertEqual(actual["maker"][1]["name"], "claude-opus-5-5")
        self.assertEqual(actual["maker"][1]["effort"], "high")
        self.assertEqual(actual["maker"][1]["budget_tokens"], 128000)

    def binary_bundle(self, directory):
        directory.mkdir(parents=True, exist_ok=True)
        sums = []
        for name in ("tl3", "tl3d", "tl3-deploy", "tl3-worker-supervisor"):
            content = f"release fixture for {name}\n".encode()
            deploy.write_file(directory / name, content, 0o700)
            sums.append(f"{hashlib.sha256(content).hexdigest()}  {name}\n")
        deploy.write_file(directory / "SHA256SUMS", "".join(sums).encode())

    def checked_bundle(self, damage=None):
        root, config, _ = deploy.paths(self.home)
        self.binary_bundle(root / "bin")
        if damage:
            damage(root / "bin")
        source, _ = self.config()
        deploy.write_file(config / "tl3.toml", source.read_bytes())
        for name in ("model-key", "hindsight-key"):
            deploy.write_file(config / "secrets" / name, b"fixture")
        deploy.write_file(config / "production-ready", b"")
        with patch.object(deploy, "mlflow_health", return_value=True), \
             patch.object(deploy, "launcher_works", return_value=(True, "fixture")), \
             patch.object(subprocess, "run", return_value=subprocess.CompletedProcess([], 0)), \
             patch.object(deploy.http.client, "HTTPConnection") as http, \
             contextlib.redirect_stdout(io.StringIO()) as output:
            http.return_value.getresponse.side_effect = [
                unittest.mock.Mock(status=200),
                unittest.mock.Mock(status=405),
            ]
            result = deploy.check(self.home)
        return result, output.getvalue()

    def test_check_accepts_complete_binary_bundle(self):
        result, output = self.checked_bundle()
        self.assertEqual(result, 0, output)

    def test_check_rejects_truncated_binary(self):
        result, output = self.checked_bundle(lambda directory: (directory / "tl3d").write_bytes(b""))
        self.assertEqual(result, 1, output)
        self.assertIn("MISSING bin/tl3d", output)

    def test_check_rejects_missing_binary(self):
        result, output = self.checked_bundle(lambda directory: (directory / "tl3-deploy").unlink())
        self.assertEqual(result, 1, output)
        self.assertIn("MISSING bin/tl3-deploy", output)

    def test_check_rejects_stale_binary(self):
        result, output = self.checked_bundle(
            lambda directory: (directory / "tl3-worker-supervisor").write_bytes(b"older release"))
        self.assertEqual(result, 1, output)
        self.assertIn("MISSING bin/tl3-worker-supervisor", output)

    def test_check_ignores_binaries_owned_by_other_deployers(self):
        # tl3-console is installed into bin/ by RobTand/tl3-console's own deploy and is not
        # in TL3's release SHA256SUMS; TL3's check must neither verify nor flag it (#784 regression).
        for name in ("tl3-console", "old-worker"):
            result, output = self.checked_bundle(
                lambda directory, name=name: deploy.write_file(directory / name, b"other deployer", 0o700))
            self.assertEqual(result, 0, output)
            self.assertNotIn(f"bin/{name}", output)

    def test_check_requires_every_release_binary_in_the_manifest(self):
        def drop(directory):
            sums = (directory / "SHA256SUMS").read_text().splitlines(keepends=True)
            (directory / "SHA256SUMS").write_text("".join(l for l in sums if not l.rstrip().endswith(" tl3d")))
        result, output = self.checked_bundle(drop)
        self.assertGreaterEqual(result, 1, output)
        self.assertIn("MISSING bin/tl3d", output)

    def test_failed_binary_skips_only_checks_that_invoke_it(self):
        result, output = self.checked_bundle(lambda directory: (directory / "tl3").write_bytes(b"stale"))
        self.assertIn("MISSING bin/tl3:", output)
        self.assertIn("MISSING tl3 status: binary verification failed", output)
        self.assertNotIn("Postgres reachable and migrated: binary verification failed", output)
        self.assertNotIn("Config::load including secret metadata: binary verification failed", output)
        self.assertEqual(result, 1, output)
        self.assertIn("Deployment check: 2 missing checks", output)

    def test_install_preserves_release_binary_manifest(self):
        binaries = self.home / "release"
        self.binary_bundle(binaries)
        args = deploy.argparse.Namespace(home=self.home, binaries=binaries,
                                        status_listen="192.168.1.107:8033", image=None,
                                        primary=None, alternate=None, console_listen=None,
                                        managed_tunnel=False)
        with patch.object(deploy, "postgres"), patch.object(deploy, "checked_download"), \
             patch.object(deploy, "install_release", return_value=False), \
             patch.object(deploy, "configure"), patch.object(deploy, "grant_console_reader"), \
             patch.object(deploy, "collect_releases"), patch.object(deploy, "run"), \
             contextlib.redirect_stdout(io.StringIO()):
            deploy.install(args)
        installed = deploy.paths(self.home)[0] / "bin/SHA256SUMS"
        self.assertTrue(installed.is_file(), "install must retain the release SHA256SUMS")
        self.assertEqual(installed.read_bytes(), (binaries / "SHA256SUMS").read_bytes())

    def test_check_reports_both_bad_credentials_as_missing(self):
        config, data = self.config()
        deploy.write_file(deploy.paths(self.home)[0] / "deploy/managed-tunnel", b"managed\n")
        destination = self.home / ".config/tl3/tl3.toml"
        deploy.write_file(destination, config.read_bytes())
        key = destination.parent / "secrets/model-key"
        token = destination.parent / "secrets/cloudflared-token"
        # tl3d's unit loads hindsight-key unconditionally, so a missing file stops the service.
        hindsight = destination.parent / "secrets/hindsight-key"
        for content, mode in ((b"", 0o600), (b"fixture", 0o644), (b"fixture", 0o600)):
            for path in (key, token, hindsight):
                deploy.write_file(path, content, mode)
            with patch.object(subprocess, "run", return_value=subprocess.CompletedProcess([], 0)), \
                 patch.object(deploy.http.client, "HTTPConnection"), \
                 contextlib.redirect_stdout(io.StringIO()) as output:
                deploy.check(self.home)
            prefix = "OK" if content and mode == 0o600 else "MISSING"
            self.assertIn(f"{prefix} model credential\n", output.getvalue())
            self.assertIn(f"{prefix} Cloudflare token\n", output.getvalue())
            self.assertIn(f"{prefix} Hindsight credential\n", output.getvalue())

    def test_external_tunnel_needs_no_token_or_tl3_connector(self):
        config, _ = self.config()
        destination = self.home / ".config/tl3/tl3.toml"
        deploy.write_file(destination, config.read_bytes())
        deploy.write_file(destination.parent / "secrets/model-key", b"fixture")
        deploy.write_file(destination.parent / "production-ready", b"")
        calls = []

        def command(argv, **kwargs):
            calls.append(list(map(str, argv)))
            return subprocess.CompletedProcess(argv, 0)

        with patch.object(subprocess, "run", side_effect=command), \
             patch.object(deploy.http.client, "HTTPConnection") as http, \
             contextlib.redirect_stdout(io.StringIO()) as output:
            http.return_value.getresponse.return_value.status = 200
            deploy.check(self.home)
        self.assertNotIn("Cloudflare token", output.getvalue())
        self.assertFalse(any("tl3-cloudflared" in call for call in calls))
        self.assertIn("external webhook route", output.getvalue())

    def test_tl3_leaves_the_console_to_its_own_repository(self):
        # PRD #327: RobTand/tl3-console builds, installs and runs tl3-console.service.
        # TL3 keeps the board projection and the console's read-only login.
        self.assertFalse((deploy.HERE / "units/tl3-console.service").exists())
        self.assertNotIn("tl3-console", (deploy.HERE / "build-release.sh").read_text())
        self.assertFalse(hasattr(deploy, "build_console_assets"))
        config, _ = self.config()
        deploy.write_file(self.home / ".config/tl3/tl3.toml", config.read_bytes())
        runtime, credentials = self.home / "runtime", self.home / "credentials"
        with patch.dict(os.environ, {"RUNTIME_DIRECTORY": str(runtime), "CREDENTIALS_DIRECTORY": str(credentials)}), \
             patch.object(os, "execv") as execv, self.assertRaises(RuntimeError):
            deploy.service(self.home, "console")
        execv.assert_not_called()
        calls, probes = [], []

        def command(argv, **kwargs):
            calls.append(list(map(str, argv)))
            return subprocess.CompletedProcess(argv, 0)

        def connection(host, port, **kwargs):
            probes.append(f"{host}:{port}")
            return unittest.mock.MagicMock()

        with patch.object(subprocess, "run", side_effect=command), \
             patch.object(deploy.http.client, "HTTPConnection", side_effect=connection), \
             contextlib.redirect_stdout(io.StringIO()) as output:
            deploy.check(self.home)
        self.assertFalse(any("tl3-console" in part for call in calls for part in call))
        self.assertNotIn("console", output.getvalue())
        self.assertNotIn("192.168.1.107:8034", probes)

    def test_controller_stops_its_whole_process_group_and_preserves_gate(self):
        unit = (deploy.HERE / "units/tl3d.service").read_text().splitlines()
        self.assertIn("KillMode=control-group", unit)
        self.assertIn("ConditionPathExists=%h/.config/tl3/production-ready", unit)

    def test_generated_config_serves_the_console_on_the_lan(self):
        _, data = self.config()
        self.assertEqual(data["console"]["listen"], "192.168.1.107:8034")
        self.assertEqual(data["console"]["assets"], str(self.home / ".local/share/tl3/console"))
        with self.assertRaises(RuntimeError):
            deploy.render_config(self.home, "192.168.1.107:8033", console_listen="127.0.0.1:8034")

    def test_config_migration_adds_the_console_and_keeps_operator_values(self):
        config, _ = self.config()
        destination = self.home / ".config/tl3/tl3.toml"
        text = config.read_text()
        start = text.index("[console]")
        without = text[:start] + text[text.index("\n[", start) + 1:]
        for original, listen in ((without, "192.168.1.107:8034"),
                                 (text.replace('"192.168.1.107:8034"', '"192.168.1.107:9000"'), "192.168.1.107:9000")):
            with self.subTest(listen=listen):
                deploy.write_file(destination, original.encode())
                deploy.configure(self.home, "192.168.1.107:8033", {})
                actual = tomllib.loads(destination.read_text())["console"]
                self.assertEqual(actual["listen"], listen)
                self.assertEqual(actual["assets"], str(self.home / ".local/share/tl3/console"))
                parsed = subprocess.run([self.helper, "parse", destination], capture_output=True)
                self.assertEqual(parsed.returncode, 0, parsed.stderr.decode())

    def test_fresh_install_uses_the_console_listen_flag(self):
        destination = self.home / ".config/tl3/tl3.toml"
        deploy.configure(self.home, "192.168.1.107:8033", {"installation_id": 987},
                         console_listen="192.168.1.107:9100")
        self.assertEqual(tomllib.loads(destination.read_text())["console"]["listen"], "192.168.1.107:9100")

    def test_console_connects_as_a_read_only_login(self):
        _, data = self.config()
        config = self.home / ".config/tl3"
        self.assertEqual(data["console"]["user"], "tl3_console")
        self.assertEqual(data["console"]["password_file"], str(config / "secrets/console-postgres-password"))
        destination = config / "tl3.toml"
        text = self.config()[0].read_text()
        older = "\n".join(line for line in text.splitlines()
                          if not line.startswith(("user = \"tl3_console\"", "password_file = \"" + str(config / "secrets/console"))))
        deploy.write_file(destination, (older + "\n").encode())
        deploy.configure(self.home, "192.168.1.107:8033", {})
        console = tomllib.loads(destination.read_text())["console"]
        self.assertEqual(console["user"], "tl3_console")
        self.assertEqual(console["password_file"], str(config / "secrets/console-postgres-password"))
        # A missing console credential must never stop Postgres, which tl3d and status require.
        self.assertNotIn("console-postgres-password", (deploy.HERE / "units/tl3-postgres.service").read_text())

    def test_postgres_setup_creates_the_console_credential_once(self):
        root, config, binaries = deploy.paths(self.home)
        (binaries / "bin").mkdir(parents=True)
        (binaries / "bin/postgres").write_text("")
        with contextlib.redirect_stdout(io.StringIO()):
            deploy.postgres(self.home)
        secret = config / "secrets/console-postgres-password"
        deploy.private_file(secret)
        first = secret.read_bytes()
        self.assertNotEqual(first, (config / "secrets/postgres-password").read_bytes())
        with contextlib.redirect_stdout(io.StringIO()):
            deploy.postgres(self.home)
        self.assertEqual(secret.read_bytes(), first)

    def test_database_reads_the_console_credential_in_place(self):
        config, _ = self.config()
        destination = self.home / ".config/tl3/tl3.toml"
        deploy.write_file(destination, config.read_bytes())
        runtime = self.home / "runtime"
        credentials = self.home / "credentials"
        secret = self.home / ".config/tl3/secrets/console-postgres-password"
        with patch.dict(os.environ, {"RUNTIME_DIRECTORY": str(runtime), "CREDENTIALS_DIRECTORY": str(credentials)}), \
             patch.object(os, "execv"):
            deploy.service(self.home, "database")
        written = tomllib.loads((runtime / "tl3.toml").read_text())
        # The database service reads the console secret in place; the grant is best effort there.
        self.assertEqual(written["console"]["password_file"], str(secret))

    def test_install_grants_the_console_login_when_postgres_runs(self):
        root, config, _ = deploy.paths(self.home)
        for active in (True, False):
            calls = []

            def command(argv, **kwargs):
                calls.append([str(part) for part in argv])
                return subprocess.CompletedProcess(argv, 0 if active else 3)

            with self.subTest(active=active), patch.object(subprocess, "run", side_effect=command), \
                 patch.object(deploy, "run", side_effect=lambda argv, **kwargs: calls.append([str(p) for p in argv])), \
                 contextlib.redirect_stdout(io.StringIO()) as output:
                deploy.grant_console_reader(self.home)
            grant = [str(root / "bin/tl3-deploy"), "console-role", str(config / "tl3.toml")]
            self.assertEqual(grant in calls, active)
            if not active:
                self.assertIn("tl3-postgres", output.getvalue())

    def release_source(self, version):
        source = self.home / f"source-{version}"
        (source / "workers/src/tl3_workers").mkdir(parents=True, exist_ok=True)
        (source / "harness").mkdir(exist_ok=True)
        (source / "workers/pyproject.toml").write_text("[project]\nname = 'tl3-workers'\n")
        (source / "workers/src/tl3_workers/__init__.py").write_text(f"VERSION = {version}\n")
        (source / "harness/runner.py").write_text("# harness\n")
        # Ignored build products never change a release's identity.
        (source / "workers/.venv").mkdir(exist_ok=True)
        (source / "workers/.venv/marker").write_text(str(version))
        return source

    def install_release(self, source):
        synced = []

        def fake_sync(argv, **kwargs):
            project = Path(argv[argv.index("--project") + 1])
            (project / ".venv/bin").mkdir(parents=True)
            (project / ".venv/bin/python").symlink_to("/usr/bin/python3")
            synced.append(project)

        with patch.object(deploy, "run", side_effect=fake_sync), contextlib.redirect_stdout(io.StringIO()):
            introduced = deploy.install_release(self.home, source)
        return introduced, synced

    def collect(self, introduced=False, runtime=None):
        environment = {"XDG_RUNTIME_DIR": str(runtime or self.home / "no-runtime")}
        with patch.dict(os.environ, environment), contextlib.redirect_stdout(io.StringIO()) as output:
            deploy.collect_releases(self.home, introduced)
        return output.getvalue()

    def live_attempt(self, python, *, launch=True):
        runs = deploy.paths(self.home)[0] / "worker-runs"
        attempt = runs / f"run{len(list(runs.glob('*')))}" / "attempt-00000000"
        attempt.mkdir(parents=True)
        stat = Path("/proc/self/stat").read_text().rsplit(")", 1)[1].split()
        (attempt / "claim.json").write_text(json.dumps({
            "pid": os.getpid(), "start": int(stat[19]),
            "machine": Path("/etc/machine-id").read_text().strip(),
            "boot": Path("/proc/sys/kernel/random/boot_id").read_text().strip()}))
        if launch:
            (attempt / "launch.json").write_text(json.dumps({"python": str(python)}))
        return attempt

    def test_running_worker_keeps_its_release_across_deploys(self):
        root = deploy.paths(self.home)[0]
        root.mkdir(parents=True)
        self.assertTrue(self.install_release(self.release_source(1))[0], "the first install introduces releases")
        first = (root / "current").resolve()
        self.live_attempt(first / "workers/.venv/bin/python")
        self.assertFalse(self.install_release(self.release_source(2))[0])
        second = (root / "current").resolve()
        self.assertEqual((root / "previous").resolve(), first)
        # The running worker's release is untouched; only `current` moved.
        self.assertEqual((first / "workers/src/tl3_workers/__init__.py").read_text(), "VERSION = 1\n")
        self.assertEqual((second / "workers/src/tl3_workers/__init__.py").read_text(), "VERSION = 2\n")
        self.assertFalse((second / "workers/.venv/marker").exists())
        self.install_release(self.release_source(3))
        self.collect()
        self.assertEqual({path.resolve() for path in (root / "releases").iterdir()},
                         {first, second, (root / "current").resolve()},
                         "keep current, previous and the live run's release")
        for record in (root / "worker-runs").glob("*/attempt-*/claim.json"):
            record.unlink()
        third = (root / "current").resolve()
        self.install_release(self.release_source(4))
        self.collect()
        self.assertEqual({path.resolve() for path in (root / "releases").iterdir()},
                         {third, (root / "current").resolve()})

    def test_redeploying_the_same_code_reuses_its_release_and_keeps_previous(self):
        root = deploy.paths(self.home)[0]
        root.mkdir(parents=True)
        self.install_release(self.release_source(1))
        first = (root / "current").resolve()
        source = self.release_source(2)
        self.install_release(source)
        release = (root / "current").resolve()
        before = (release / ".installed").stat().st_mtime_ns
        introduced, synced = self.install_release(source)
        self.assertFalse(introduced)
        self.assertEqual(synced, [], "no copy or sync over a running release")
        self.assertEqual((root / "current").resolve(), release)
        self.assertEqual((release / ".installed").stat().st_mtime_ns, before)
        self.assertEqual((root / "previous").resolve(), first, "a redeploy keeps the real previous release")
        self.collect()
        self.assertTrue(first.is_dir())

    def test_unfinished_release_is_rebuilt_not_used(self):
        root = deploy.paths(self.home)[0]
        root.mkdir(parents=True)
        source = self.release_source(1)
        partial = root / "releases" / deploy.release_id(source)
        (partial / "workers").mkdir(parents=True)
        (partial / "workers/stale.py").write_text("partial copy\n")
        _, synced = self.install_release(source)
        self.assertEqual(synced, [partial / "workers"])
        self.assertFalse((partial / "workers/stale.py").exists())
        self.assertTrue((partial / ".installed").exists())

    def migrated_config(self, python):
        _, config_dir, _ = deploy.paths(self.home)
        config, _ = self.config()
        text = config.read_text().replace(
            json.dumps(str(deploy.paths(self.home)[0] / "current/workers/.venv/bin/python")), json.dumps(python))
        deploy.write_file(config_dir / "tl3.toml", text.encode())

    def test_pre_372_app_directory_outlives_everything_that_can_use_it(self):
        root = deploy.paths(self.home)[0]
        (root / "app/workers").mkdir(parents=True)
        legacy_python = str(root / "app/workers/.venv/bin/python")
        current_python = str(root / "current/workers/.venv/bin/python")
        self.install_release(self.release_source(1))
        self.migrated_config(current_python)
        self.assertIn("this install introduced releases", self.collect(introduced=True))
        self.assertTrue((root / "app").is_dir(), "the tl3d running during the first install still uses app/")
        legacy = self.live_attempt(legacy_python, launch=False)
        self.assertIn("a run started before #372 is live", self.collect())
        (legacy / "claim.json").unlink()
        self.migrated_config(legacy_python)
        self.assertIn("tl3.toml does not run workers", self.collect())
        self.migrated_config(current_python)
        runtime = self.home / "runtime"
        (runtime / "tl3d").mkdir(parents=True)
        (runtime / "tl3d/tl3.toml").write_text(f'[workers]\npython = {json.dumps(legacy_python)}\n')
        self.assertIn("the running tl3d launches workers from app/", self.collect(runtime=runtime))
        self.assertTrue((root / "app").is_dir())
        (runtime / "tl3d/tl3.toml").write_text(f'[workers]\npython = {json.dumps(current_python)}\n')
        self.collect(runtime=runtime)
        self.assertFalse((root / "app").exists())

    def test_supervisors_launch_outside_tl3d_from_the_current_release(self):
        config, _ = self.config()
        workers = tomllib.loads(config.read_text())["workers"]
        self.assertEqual(workers["launcher"][:3], ["/usr/bin/systemd-run", "--user", "--scope"])
        self.assertIn("--slice=tl3-workers.slice", workers["launcher"])
        self.assertEqual(workers["python"], str(deploy.paths(self.home)[0] / "current/workers/.venv/bin/python"))
        self.assertTrue((deploy.HERE / "units" / deploy.WORKER_SLICE).is_file())
        unit = (deploy.HERE / "units/tl3d.service").read_text().splitlines()
        self.assertIn("KillMode=control-group", unit, "tl3d's own cgroup still stops on its own")

    def live_shaped_config(self, python):
        """A pre-#372 tl3.toml as it is on dl380g10: the app/ line, models and per-repo protection."""
        config, _ = self.config()
        launcher = "launcher = " + deploy.toml_value(deploy.LAUNCHER) + "\n"
        current = 'python = ' + json.dumps(str(deploy.paths(self.home)[0] / "current/workers/.venv/bin/python"))
        text = config.read_text().replace(launcher, "").replace(current, f'python = {json.dumps(python)}')
        text = text.replace("[[github.repos]]\n", '[[github.repos]]\nprotected_paths = ["deploy/**", "CHANGELOG.md"]\n', 1)
        self.assertIn(f'python = {json.dumps(python)}\n', text)
        self.assertIn("\n[models]\n", text)
        return text

    def test_config_migration_moves_only_the_deploy_owned_python_and_is_idempotent(self):
        root, config_dir, _ = deploy.paths(self.home)
        destination = config_dir / "tl3.toml"
        legacy = "/home/rob/.local/share/tl3/app/workers/.venv/bin/python".replace("/home/rob", str(self.home))
        for python, expected in ((legacy, str(root / "current/workers/.venv/bin/python")),
                                 ("/opt/tl3/python", "/opt/tl3/python")):
            with self.subTest(python=python):
                for backup in config_dir.glob("tl3.toml.*.bak"):
                    backup.unlink()
                original = self.live_shaped_config(python)
                deploy.write_file(destination, original.encode())
                with contextlib.redirect_stdout(io.StringIO()):
                    deploy.configure(self.home, "192.168.1.107:8033", {"installation_id": 987, "operations_issue": 123})
                migrated = destination.read_text()
                workers = tomllib.loads(migrated)["workers"]
                self.assertEqual(workers["python"], expected)
                self.assertEqual(workers["launcher"], deploy.LAUNCHER)
                # Every other line is byte-identical: only the python line and the launcher change.
                expected_lines = original.replace(f'python = {json.dumps(python)}', f'python = {json.dumps(expected)}')
                expected_lines = expected_lines.replace("[workers]\n", "[workers]\nlauncher = "
                                                        + deploy.toml_value(deploy.LAUNCHER) + "\n", 1)
                self.assertEqual(migrated, expected_lines)
                self.assertEqual(len(list(config_dir.glob("tl3.toml.*.bak"))), 1)
                info = destination.stat()
                with contextlib.redirect_stdout(io.StringIO()):
                    deploy.configure(self.home, "192.168.1.107:8033", {"installation_id": 987, "operations_issue": 123})
                self.assertEqual(destination.read_text(), migrated, "a second run changes nothing")
                self.assertEqual(destination.stat().st_mtime_ns, info.st_mtime_ns)
                self.assertEqual(len(list(config_dir.glob("tl3.toml.*.bak"))), 1, "and writes no backup")

    def test_config_migration_fails_loudly_when_the_python_line_does_not_match(self):
        root, config_dir, _ = deploy.paths(self.home)
        legacy = str(root / "app/workers/.venv/bin/python")
        original = self.live_shaped_config(legacy).replace(f'python = {json.dumps(legacy)}',
                                                           f'python={json.dumps(legacy)}')
        destination = config_dir / "tl3.toml"
        deploy.write_file(destination, original.encode())
        with self.assertRaisesRegex(RuntimeError, "could not move workers.python"), \
             contextlib.redirect_stdout(io.StringIO()):
            deploy.configure(self.home, "192.168.1.107:8033", {"installation_id": 987, "operations_issue": 123})
        self.assertEqual(destination.read_text(), original)

    def test_stop_runs_refuses_unless_tl3d_is_inactive_or_failed(self):
        for state, allowed in (("active", False), ("activating", False), ("reloading", False),
                               ("deactivating", False), ("inactive", True), ("failed", True)):
            calls = []
            with self.subTest(state=state), patch.object(deploy, "_tl3d_state", return_value=state), \
                 patch.object(deploy, "run", side_effect=lambda argv, **kwargs: calls.append([str(p) for p in argv])), \
                 contextlib.redirect_stdout(io.StringIO()):
                if allowed:
                    deploy.stop_runs(self.home)
                else:
                    with self.assertRaisesRegex(RuntimeError, "Stop tl3d first"):
                        deploy.stop_runs(self.home)
            self.assertEqual(calls, [["systemctl", "--user", "stop", "tl3-workers.slice"]] if allowed else [])

    def test_app_is_kept_while_tl3d_runs_without_a_readable_runtime_config(self):
        root = deploy.paths(self.home)[0]
        (root / "app/workers").mkdir(parents=True)
        self.install_release(self.release_source(1))
        self.migrated_config(str(root / "current/workers/.venv/bin/python"))
        runtime = self.home / "runtime"
        (runtime / "tl3d").mkdir(parents=True)
        for state in ("activating", "active", "reloading"):
            for content in (None, "not = [toml"):
                path = runtime / "tl3d/tl3.toml"
                path.unlink(missing_ok=True)
                if content is not None:
                    path.write_text(content)
                with self.subTest(state=state, content=content), \
                     patch.object(deploy, "_tl3d_state", return_value=state):
                    self.assertIn("tl3d is running without a readable runtime config", self.collect(runtime=runtime))
                    self.assertTrue((root / "app").is_dir())
        (runtime / "tl3d/tl3.toml").unlink()
        with patch.object(deploy, "_tl3d_state", return_value="inactive"):
            self.collect(runtime=runtime)
        self.assertFalse((root / "app").exists(), "a stopped tl3d launches nothing from app/")

    def probe_config(self, launcher):
        _, config_dir, _ = deploy.paths(self.home)
        config, _ = self.config()
        text = config.read_text().replace("launcher = " + deploy.toml_value(deploy.LAUNCHER),
                                          "launcher = " + deploy.toml_value(launcher))
        deploy.write_file(config_dir / "tl3.toml", text.encode())

    def probe(self, *, timeout=20):
        runtime = self.home / "runtime"
        runtime.mkdir(exist_ok=True)
        bus = runtime / "bus"
        if not bus.exists():
            server = socket.socket(socket.AF_UNIX)
            self.addCleanup(server.close)
            server.bind(str(bus))
        with patch.dict(os.environ, {"XDG_RUNTIME_DIR": str(runtime)}), \
             patch.object(deploy, "PROBE_TIMEOUT", timeout, create=True):
            return deploy.launcher_works(self.home)

    def fake_launcher(self, body):
        script = self.home / "fake-launcher"
        script.write_text("#!/bin/sh\n" + body + "\n")
        script.chmod(0o700)
        return [str(script)]

    def test_launcher_probe_runs_the_configured_launcher_and_reports_why_it_failed(self):
        self.probe_config(self.fake_launcher('echo "Failed to connect to user scope bus" >&2; exit 1'))
        ok, detail = self.probe()
        self.assertFalse(ok)
        self.assertIn("Failed to connect to user scope bus", detail, "the configured launcher ran, not the constant")

    def test_launcher_probe_is_bounded(self):
        self.probe_config(self.fake_launcher("exec sleep 30"))
        started = time.monotonic()
        ok, detail = self.probe(timeout=1)
        self.assertFalse(ok)
        self.assertIn("timed out", detail)
        self.assertLess(time.monotonic() - started, 15)

    def test_launcher_probe_needs_a_scope_of_its_own_and_a_working_cgroup_kill(self):
        # A launcher that runs the command in place leaves it in the caller's cgroup.
        self.probe_config(["/usr/bin/env"])
        ok, detail = self.probe()
        self.assertFalse(ok)
        self.assertIn("scope of its own", detail)
        pid_file = self.home / "probe.pid"
        self.probe_config(self.fake_launcher(
            f'echo $$ > {pid_file}; echo "0::/user.slice/user@1000.service/tl3.slice/tl3-workers.slice/run-probe.scope"; '
            "exec sleep 30"))
        killed = []

        def cgroup_kill(scope):
            killed.append(scope)
            os.kill(int(pid_file.read_text()), 9)

        with patch.object(deploy, "_kill_cgroup", side_effect=cgroup_kill):
            ok, detail = self.probe()
        self.assertTrue(ok, detail)
        self.assertEqual(killed, [Path("/sys/fs/cgroup/user.slice/user@1000.service/tl3.slice/tl3-workers.slice/run-probe.scope")])
        with patch.object(deploy, "_kill_cgroup", side_effect=lambda scope: None):
            ok, detail = self.probe(timeout=1)
        self.assertFalse(ok)
        self.assertIn("cgroup.kill", detail)

    def test_launcher_probe_needs_the_user_bus_and_a_configured_launcher(self):
        with patch.dict(os.environ, {"XDG_RUNTIME_DIR": str(self.home / "nothing")}):
            ok, detail = deploy.launcher_works(self.home)
        self.assertFalse(ok)
        self.assertIn("user bus", detail)
        self.probe_config([])
        ok, detail = self.probe()
        self.assertFalse(ok)
        self.assertIn("workers.launcher", detail)

    def test_command_failure_never_prints_child_output(self):
        with contextlib.redirect_stdout(io.StringIO()) as output:
            with self.assertRaisesRegex(RuntimeError, "output suppressed"):
                deploy.run(["sh", "-c", "echo secret-output; exit 7"])
        self.assertEqual(output.getvalue(), "")


if __name__ == "__main__":
    unittest.main()
