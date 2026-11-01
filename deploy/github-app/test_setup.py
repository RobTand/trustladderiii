import copy
from html.parser import HTMLParser
from http.server import BaseHTTPRequestHandler, HTTPServer
import importlib.util
import json
import os
from pathlib import Path
import selectors
import shutil
import stat
import subprocess
import tempfile
import threading
import unittest
from urllib.error import HTTPError
from urllib.parse import parse_qs, urlencode, urlsplit
from urllib.request import Request, urlopen


ROOT = Path(__file__).parent
SPEC = importlib.util.spec_from_file_location("tl3_app_setup", ROOT / "setup.py")
setup = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(setup)
FAKE = {
    "id": 123,
    "slug": "tl3-robtand",
    "owner": {"login": "RobTand"},
    "pem": "FAKE-PRIVATE-KEY-never-log",
    "webhook_secret": "FAKE-WEBHOOK-SECRET-never-log",
    "client_id": "FAKE-CLIENT-ID",
    "client_secret": "FAKE-CLIENT-SECRET-never-log",
}


class FormParser(HTMLParser):
    def handle_starttag(self, tag, attributes):
        attrs = dict(attributes)
        if tag == "form":
            self.form = attrs
        if tag == "input" and attrs.get("name") == "manifest":
            self.manifest = json.loads(attrs["value"])


class WizardTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.config = Path(self.temp.name) / "tl3"
        self.requests = []
        self.reply = copy.deepcopy(FAKE)
        self.status = 201
        self.redirect = None
        test = self

        class FakeAPI(BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def do_POST(self):
                test.requests.append((self.path, dict(self.headers)))
                body = json.dumps(test.reply).encode()
                self.send_response(test.status)
                if test.redirect:
                    self.send_header("Location", test.redirect)
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)

        self.api = HTTPServer(("127.0.0.1", 0), FakeAPI)
        thread = threading.Thread(target=self.api.serve_forever, daemon=True)
        thread.start()
        self.addCleanup(self.api.server_close)
        self.addCleanup(thread.join)
        self.addCleanup(self.api.shutdown)
        self.api_url = f"http://127.0.0.1:{self.api.server_port}"

    def start(self):
        self.process = subprocess.Popen(
            ["bash", str(ROOT / "wizard.sh"), "--port", "0", "--config-dir",
             str(self.config), "--fake-api-url", self.api_url],
            stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            env={**os.environ, "PYTHONDONTWRITEBYTECODE": "1"})
        self.addCleanup(self.stop)
        # Read raw bytes to avoid a buffered readline hiding readiness from select.
        self.output = b""
        with selectors.DefaultSelector() as selector:
            selector.register(self.process.stdout, selectors.EVENT_READ)
            while b"Sign in as RobTand, continue" not in self.output:
                self.assertTrue(selector.select(10), "wizard did not reach the browser step")
                chunk = os.read(self.process.stdout.fileno(), 4096)
                self.assertTrue(chunk, "wizard exited before the browser step")
                self.output += chunk
        line = next(line for line in self.output.decode().splitlines()
                    if line.startswith("Open on celestia: "))
        self.form_url = line.removeprefix("Open on celestia: ")
        self.base = self.form_url.rsplit("/", 1)[0]
        self.state = self.form_url.rsplit("/", 1)[1]

    def stop(self):
        if self.process.poll() is None:
            self.process.terminate()
        self.process.communicate(timeout=10)

    def callback(self, **values):
        query = {"state": self.state, "code": "fake-once-only-code", **values}
        return self.base + "/callback?" + urlencode(query, doseq=True)

    def finish(self, exit_code):
        out, err = self.process.communicate(timeout=10)
        self.assertEqual(exit_code, self.process.returncode)
        output = self.output + out + err
        for name in ("pem", "webhook_secret", "client_secret"):
            self.assertNotIn(FAKE[name].encode(), output)
        self.assertNotIn(b"fake-once-only-code", output)
        return output.decode()

    def test_manifest_shape_and_browser_form(self):
        self.start()
        with urlopen(self.form_url, timeout=5) as response:
            parser = FormParser()
            parser.feed(response.read().decode())
            self.assertEqual("no-store", response.headers["Cache-Control"])
            self.assertEqual("no-referrer", response.headers["Referrer-Policy"])
        self.assertEqual("post", parser.form["method"])
        action = urlsplit(parser.form["action"])
        self.assertEqual(("https", "github.com", "/settings/apps/new"),
                         (action.scheme, action.netloc, action.path))
        self.assertEqual({"state": [self.state]}, parse_qs(action.query))
        manifest = parser.manifest
        self.assertEqual({"name", "url", "description", "public", "redirect_url",
                          "hook_attributes", "default_permissions", "default_events"}, set(manifest))
        self.assertIs(manifest["public"], False)
        self.assertIn("TL3", manifest["name"])
        self.assertEqual(self.base + "/callback", manifest["redirect_url"])
        self.assertEqual({"url": "https://tl3-webhook.invalid/github/events", "active": False},
                         manifest["hook_attributes"])
        self.assertEqual({"issues": "write", "pull_requests": "write", "checks": "write",
                          "contents": "write", "metadata": "read", "merge_queues": "read",
                          "actions": "write", "administration": "read"},
                         manifest["default_permissions"])
        self.assertEqual({"issues", "issue_comment", "pull_request", "check_run",
                          "check_suite", "merge_group"}, set(manifest["default_events"]))
        self.assertEqual([], self.requests)
        self.assertFalse((self.config / "github-app.json").exists())

    def test_fake_conversion_saves_credentials_and_install_link(self):
        self.config.mkdir(mode=0o755)
        self.start()
        with urlopen(self.callback(), timeout=5) as response:
            body = response.read().decode()
            for name in ("pem", "webhook_secret", "client_secret"):
                self.assertNotIn(FAKE[name], body)
            self.assertIn("Only select repositories", body)
            self.assertIn("tl3-sandbox only", body)
        output = self.finish(0)
        install = "https://github.com/apps/tl3-robtand/installations/new"
        self.assertIn(install, output)
        credentials = self.config / "github-app.json"
        self.assertEqual({k: v for k, v in FAKE.items() if k != "owner"},
                         json.loads(credentials.read_text()))
        self.assertEqual(0o700, stat.S_IMODE(self.config.stat().st_mode))
        self.assertEqual(0o600, stat.S_IMODE(credentials.stat().st_mode))
        self.assertEqual(["github-app.json"], os.listdir(self.config))
        self.assertEqual(1, len(self.requests))
        path, headers = self.requests[0]
        self.assertEqual("/app-manifests/fake-once-only-code/conversions", path)
        self.assertNotIn("Authorization", headers)

    def test_invalid_callbacks_and_hosts_never_convert(self):
        self.start()
        callbacks = [self.callback(state="wrong"), self.callback(state=""),
                     self.callback(state="é"), self.callback(code=""),
                     self.callback(code="../escape"), self.callback(code=["a", "b"]),
                     self.callback(state=[self.state, self.state])]
        callbacks.append(self.base + "/callback?code=missing-state")
        for url in callbacks:
            with self.subTest(url=url), self.assertRaises(HTTPError) as error:
                urlopen(url, timeout=5)
            self.assertEqual(400, error.exception.code)
            error.exception.close()
        with self.assertRaises(HTTPError) as error:
            urlopen(Request(self.form_url, headers={"Host": "hostile.example"}), timeout=5)
        self.assertEqual(403, error.exception.code)
        error.exception.close()
        self.assertEqual([], self.requests)
        self.assertFalse((self.config / "github-app.json").exists())

    def assert_conversion_failure(self):
        self.start()
        with self.assertRaises(HTTPError) as error:
            urlopen(self.callback(), timeout=5)
        self.assertEqual(502, error.exception.code)
        body = error.exception.read().decode()
        error.exception.close()
        self.assertNotIn(FAKE["client_secret"], body)
        self.finish(1)
        self.assertFalse((self.config / "github-app.json").exists())

    def test_upstream_error_is_redacted(self):
        self.status = 422
        self.assert_conversion_failure()

    def test_conversion_redirect_is_not_followed(self):
        self.status = 307
        self.redirect = self.api_url + "/must-not-follow"
        self.assert_conversion_failure()
        self.assertEqual(1, len(self.requests))

    def test_missing_credentials_fail(self):
        del self.reply["webhook_secret"]
        self.assert_conversion_failure()

    def test_wrong_owner_fails(self):
        self.reply["owner"]["login"] = "another-account"
        self.assert_conversion_failure()

    def test_invalid_slug_is_not_rendered(self):
        self.reply["slug"] = '\"><script>bad</script>'
        self.assert_conversion_failure()

    def test_refuses_existing_credentials_without_reading_them(self):
        self.config.mkdir()
        existing = self.config / "github-app.json"
        existing.write_text("keep-existing")
        with self.assertRaises(setup.SetupError):
            setup.prepare_directory(self.config)
        self.assertEqual("keep-existing", existing.read_text())

    def test_refuses_symlink_directory(self):
        self.config.symlink_to(Path(self.temp.name), target_is_directory=True)
        with self.assertRaises(OSError):
            setup.prepare_directory(self.config)

    def test_refuses_credential_symlink(self):
        self.config.mkdir()
        existing = self.config / "github-app.json"
        existing.symlink_to(Path(self.temp.name) / "nonexistent")
        with self.assertRaises(setup.SetupError):
            setup.prepare_directory(self.config)

    def test_save_never_overwrites_and_cleans_temporary_file(self):
        fd = setup.prepare_directory(self.config)
        self.addCleanup(os.close, fd)
        setup.save_credentials(fd, FAKE)
        with self.assertRaises(FileExistsError):
            setup.save_credentials(fd, {"changed": True})
        self.assertEqual(FAKE, json.loads((self.config / "github-app.json").read_text()))
        self.assertEqual(["github-app.json"], os.listdir(self.config))

    def test_fake_endpoint_override_cannot_target_github(self):
        for endpoint in ("https://api.github.com", "http://localhost:1",
                         "http://127.0.0.1:1/extra", "http://127.0.0.1:1?query=yes"):
            done = subprocess.run(["python3", str(ROOT / "setup.py"), "--fake-api-url", endpoint],
                                  capture_output=True, text=True)
            self.assertEqual(2, done.returncode)
            self.assertIn("fake API must be", done.stderr)

    def test_bash_syntax(self):
        subprocess.run(["bash", "-n", str(ROOT / "wizard.sh")], check=True)

    def test_shellcheck_when_installed(self):
        if not shutil.which("shellcheck"):
            self.skipTest("shellcheck is not installed on this worker")
        subprocess.run(["shellcheck", str(ROOT / "wizard.sh")], check=True)


if __name__ == "__main__":
    unittest.main()
