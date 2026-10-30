import importlib.util
import io
import contextlib
import hashlib
import os
from pathlib import Path
import stat
import subprocess
import tempfile
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
        self.assertEqual(runtime_config["models"]["key_file"], str(credentials / "model-key"))
        self.assertEqual(runtime_config["github"]["app_credentials_file"], str(credentials / "github-app.json"))
        self.assertEqual(execute.call_args.args[1][-1], "--workers")
        self.assertNotIn("uv", runtime_config["workers"]["python"])

    def test_unit_syntax_and_ordering(self):
        units = self.home / "units"
        units.mkdir()
        for source in (deploy.HERE / "units").glob("*.service"):
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
        result = subprocess.run(["systemd-analyze", "--user", "--man=no", "verify", *map(str, units.glob("*.service"))], capture_output=True, env=environment)
        self.assertEqual(result.returncode, 0, result.stderr.decode())
        controller = (units / "tl3d.service").read_text()
        self.assertIn("KillMode=process", controller)
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
        self.assertEqual(actual["maker"][1]["effort"], "max")
        self.assertEqual(actual["maker"][1]["budget_tokens"], 128000)

    def test_check_reports_both_bad_credentials_as_missing(self):
        config, data = self.config()
        destination = self.home / ".config/tl3/tl3.toml"
        deploy.write_file(destination, config.read_bytes())
        key = destination.parent / "secrets/model-key"
        token = destination.parent / "secrets/cloudflared-token"
        for content, mode in ((b"", 0o600), (b"fixture", 0o644), (b"fixture", 0o600)):
            for path in (key, token):
                deploy.write_file(path, content, mode)
            with patch.object(subprocess, "run", return_value=subprocess.CompletedProcess([], 0)), \
                 patch.object(deploy.http.client, "HTTPConnection"), \
                 contextlib.redirect_stdout(io.StringIO()) as output:
                deploy.check(self.home)
            prefix = "OK" if content and mode == 0o600 else "MISSING"
            self.assertIn(f"{prefix} model credential\n", output.getvalue())
            self.assertIn(f"{prefix} Cloudflare token\n", output.getvalue())

    def test_command_failure_never_prints_child_output(self):
        with contextlib.redirect_stdout(io.StringIO()) as output:
            with self.assertRaisesRegex(RuntimeError, "output suppressed"):
                deploy.run(["sh", "-c", "echo secret-output; exit 7"])
        self.assertEqual(output.getvalue(), "")


if __name__ == "__main__":
    unittest.main()
