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
            text = source.read_text().replace("%h", str(self.home))
            for line in text.splitlines():
                if line.startswith(("ExecStart=", "ExecStartPre=", "ExecStartPost=", "ExecStop=")):
                    executable = line.split("=", 1)[1].split()[0]
                    if executable.startswith(str(self.home)):
                        deploy.write_file(Path(executable), b"#!/bin/sh\nexit 0\n", 0o700)
            (units / source.name).write_text(text)
        runtime = self.home / "runtime"
        runtime.mkdir(mode=0o700)
        environment = os.environ | {"XDG_RUNTIME_DIR": str(runtime)}
        result = subprocess.run(["systemd-analyze", "--user", "--man=no", "verify", *map(str, units.glob("*.service"))], capture_output=True, env=environment)
        self.assertEqual(result.returncode, 0, result.stderr.decode())
        controller = (units / "tl3d.service").read_text()
        self.assertIn("KillMode=process", controller)
        self.assertIn("Requires=tl3-postgres.service", controller)
        self.assertIn("LoadCredential=model-key:", controller)
        cloudflared = (units / "tl3-cloudflared.service").read_text()
        self.assertIn("--token-file %d/token", cloudflared)

    def test_command_failure_never_prints_child_output(self):
        with contextlib.redirect_stdout(io.StringIO()) as output:
            with self.assertRaisesRegex(RuntimeError, "output suppressed"):
                deploy.run(["sh", "-c", "echo secret-output; exit 7"])
        self.assertEqual(output.getvalue(), "")


if __name__ == "__main__":
    unittest.main()
