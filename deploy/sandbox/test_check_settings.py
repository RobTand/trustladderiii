"""Offline settings-check coverage. Run through PrismaBuild."""

import base64
import copy
import hashlib
import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch


ROOT = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location("sandbox_settings", ROOT / "check_settings.py")
settings = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(settings)


class FakeGitHub:
    def __init__(self, approved):
        self.prefix = f"repos/{settings.REPOSITORY}"
        self.responses = {self.prefix: copy.deepcopy(approved["repository"])}
        self.responses[self.prefix + "/rulesets"] = []
        for index, rule in enumerate(approved["rulesets"]):
            self.responses[self.prefix + "/rulesets"].append({"id": index})
            self.responses[f"{self.prefix}/rulesets/{index}"] = {
                **copy.deepcopy(rule), "id": index, "source": settings.REPOSITORY,
                "source_type": "Repository", "created_at": "today", "updated_at": "today",
                "current_user_can_bypass": "never", "_links": {"self": "unused"},
            }
        self.responses[self.prefix + "/branches"] = [{"name": "main", "protected": True}]
        self.responses[self.prefix + "/branches/main/protection"] = None
        for path in settings.FILES:
            self.responses[f"{self.prefix}/contents/{path}?ref=main"] = {
                "encoding": "base64", "type": "file",
                "content": base64.b64encode((ROOT / "files" / path).read_bytes()).decode(),
            }
        self.calls = []

    def get(self, endpoint, *, missing=False):
        self.calls.append((endpoint, missing))
        result = self.responses[endpoint]
        if isinstance(result, Exception):
            raise result
        return copy.deepcopy(result)

    def pages(self, endpoint):
        return self.get(endpoint)


class CheckerTests(unittest.TestCase):
    def setUp(self):
        self.approved = json.loads((ROOT / "approved.json").read_text())
        self.github = FakeGitHub(self.approved)
        self.prefix = self.github.prefix

    def test_approved_settings_pass(self):
        self.assertEqual(settings.check(self.approved, self.github), [])
        self.assertEqual(sum(endpoint == self.prefix for endpoint, _ in self.github.calls), 1)

    def test_post_recovery_mergify_matches_approved_copy(self):
        response = json.loads((ROOT / "evidence/post-recovery/mergify-content.json").read_text())
        self.github.responses[self.prefix + "/contents/.mergify.yml?ref=main"] = response
        self.assertEqual(settings.check(self.approved, self.github), [])

    def test_each_repository_setting_is_checked(self):
        for key, value in self.approved["repository"].items():
            with self.subTest(key=key):
                github = FakeGitHub(self.approved)
                github.responses[self.prefix][key] = not value if isinstance(value, bool) else "changed"
                self.assertTrue(any(f"repository.{key}" in d for d in settings.check(self.approved, github)))

    def test_security_policy_drift_reports_all_paths(self):
        branch = self.github.responses[self.prefix + "/rulesets/0"]
        branch["bypass_actors"].append({"actor_id": 1, "actor_type": "RepositoryRole", "bypass_mode": "always"})
        branch["conditions"]["ref_name"]["include"] = ["refs/heads/wrong/**"]
        branch["enforcement"] = "disabled"
        main = self.github.responses[self.prefix + "/rulesets/1"]
        main["rules"] = [rule for rule in main["rules"] if rule["type"] != "required_status_checks"]
        changes = settings.check(self.approved, self.github)
        for field in ("bypass_actors", "include", "enforcement", "required_status_checks"):
            self.assertTrue(any(field in difference for difference in changes), field)

    def test_extra_and_missing_rulesets(self):
        self.github.responses[self.prefix + "/rulesets"].pop()
        self.assertTrue(any("rulesets" in d for d in settings.check(self.approved, self.github)))
        self.github.responses[self.prefix + "/rulesets"].append({"id": 9})
        self.github.responses[self.prefix + "/rulesets/9"] = {"name": "Unexpected policy"}
        self.assertTrue(any("Unexpected policy" in d for d in settings.check(self.approved, self.github)))

    def test_rule_and_actor_order_is_immaterial(self):
        for i in (0, 1):
            self.github.responses[f"{self.prefix}/rulesets/{i}"]["rules"].reverse()
        expected = settings.normalize(self.approved)
        actual, errors = settings.collect(self.github)
        self.assertEqual(errors, [])
        self.assertEqual(settings.differences(expected, settings.normalize(actual)), [])

    def test_each_gate_file_is_hashed_as_bytes(self):
        for path in settings.FILES:
            with self.subTest(path=path):
                github = FakeGitHub(self.approved)
                github.responses[f"{self.prefix}/contents/{path}?ref=main"]["content"] = base64.b64encode(b"weakened\r\n").decode()
                self.assertTrue(any(f"files.{path}" in d for d in settings.check(self.approved, github)))

    def test_missing_files_report_all_differences(self):
        for path in settings.FILES:
            self.github.responses[f"{self.prefix}/contents/{path}?ref=main"] = None
        changes = settings.check(self.approved, self.github)
        self.assertEqual(sum(d.startswith("files.") for d in changes), 3)

    def test_local_copies_must_match_the_reviewed_hashes(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            for name in settings.FILES:
                target = path / name
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_bytes(b"changed locally")
            changes = settings.check(self.approved, self.github, files=path)
        self.assertEqual(sum(d.startswith("local_files.") for d in changes), 3)

    def test_unreadable_settings_do_not_stop_other_reads(self):
        for path in (self.prefix, self.prefix + "/rulesets/0", self.prefix + "/branches/main/protection"):
            self.github.responses[path] = settings.ReadError("token secret must not be shown")
        for name in settings.FILES:
            self.github.responses[f"{self.prefix}/contents/{name}?ref=main"] = settings.ReadError("credential")
        changes = settings.check(self.approved, self.github)
        self.assertGreaterEqual(len(changes), 6)
        self.assertFalse(any("credential" in d or "token secret" in d for d in changes))

    def test_classic_protection_and_extra_protected_branch_detected(self):
        self.github.responses[self.prefix + "/branches"].append({"name": "extra/branch", "protected": True})
        self.github.responses[self.prefix + "/branches/extra%2Fbranch/protection"] = {"required_pull_request_reviews": {"required_approving_review_count": 2}}
        self.github.responses[self.prefix + "/branches/main/protection"] = {"enforce_admins": {"enabled": False}}
        changes = settings.check(self.approved, self.github)
        self.assertTrue(any("branch_protection.extra/branch" in d for d in changes))
        self.assertTrue(any("branch_protection.main" in d for d in changes))

    def test_app_identity_is_not_discarded_inside_protections(self):
        self.assertNotEqual(settings.normalize({"app": {"id": 1}}), settings.normalize({"app": {"id": 2}}))

    def test_missing_and_null_are_distinct(self):
        self.assertEqual(settings.differences({"a": None}, {}), ["a: missing"])
        self.assertTrue(settings.differences(False, 0))

    def test_rest_missing_only_accepts_404(self):
        github = settings.GitHub()
        for status in (401, 403, 404, 500):
            response = subprocess.CompletedProcess([], 1, f"HTTP/2.0 {status} Error\n\n{{}}", "secret")
            with self.subTest(status=status), patch.object(settings.subprocess, "run", return_value=response):
                if status == 404:
                    self.assertIsNone(github.get("endpoint", missing=True))
                else:
                    with self.assertRaises(settings.ReadError):
                        github.get("endpoint", missing=True)

    def test_rest_uses_get_only_and_never_exposes_gh_stderr(self):
        response = subprocess.CompletedProcess([], 0, 'HTTP/2.0 200 OK\nContent-Type: application/json\n\n{"ok": true}', "secret")
        with patch.object(settings.subprocess, "run", return_value=response) as run:
            self.assertEqual(settings.GitHub().get("repos/example"), {"ok": True})
        command = run.call_args.args[0]
        self.assertEqual(command[command.index("--method") + 1], "GET")
        with patch.object(settings.subprocess, "run", return_value=subprocess.CompletedProcess([], 1, "", "secret")):
            with self.assertRaisesRegex(settings.ReadError, "REST response unavailable"):
                settings.GitHub().get("endpoint")

    def test_pagination_reads_beyond_first_page(self):
        github = settings.GitHub()
        with patch.object(github, "get", side_effect=[list(range(100)), [100]]) as get:
            self.assertEqual(github.pages("endpoint"), list(range(101)))
        self.assertEqual(get.call_count, 2)

    def test_invalid_file_encoding_is_a_failure(self):
        for path in settings.FILES:
            self.github.responses[f"{self.prefix}/contents/{path}?ref=main"]["content"] = "!not-base64!"
        self.assertEqual(sum("could not read REST content" in d for d in settings.check(self.approved, self.github)), 3)

    def test_baseline_hashes_match_local_files(self):
        for path in settings.FILES:
            self.assertEqual(self.approved["files"][path], hashlib.sha256((ROOT / "files" / path).read_bytes()).hexdigest())


if __name__ == "__main__":
    unittest.main()
