"""Offline assertions for live-trial evidence. Run through PrismaBuild."""

import copy
import unittest

from trial import verify


def check(name, app_id, title=None):
    return {"id": app_id, "name": name, "head_sha": "approved",
            "app": {"id": app_id}, "status": "completed", "conclusion": "success",
            "output": {"title": title}}


def snapshots():
    original = {"number": 100, "user": {"login": "tl3-robtand[bot]"},
                "merged": False, "head": {"sha": "approved", "ref": "tl3/trial"},
                "base": {"sha": "main-before", "ref": "main"}}
    before = {"pull": original, "main": "main-before", "head-checks": [
        check("TL3 approval", 5542171), check("Sandbox tests", 15368)]}
    draft = {"pull": {"number": 101, "user": {"login": "mergify[bot]"},
                      "head": {"sha": "temporary", "ref": "mergify/merge-queue/trial"},
                      "base": {"ref": "main"}},
             "head-checks": [{**check("Sandbox tests", 15368), "head_sha": "temporary"}]}
    after = copy.deepcopy(before)
    after["pull"].update(merged=True, merge_commit_sha="merged")
    after["main"] = "merged"
    after["head-checks"].append(check("Mergify Merge Queue", 10562,
                                      "Merged via merge queue"))
    return before, draft, after


class TrialTests(unittest.TestCase):
    def test_queue_merge_with_unchanged_approved_head(self):
        self.assertEqual(verify(*snapshots()), [])

    def test_outside_queue_merge_is_rejected(self):
        before, draft, after = snapshots()
        after["head-checks"].pop()
        self.assertIn("queue merge", " ".join(verify(before, draft, after)))

    def test_unmerged_pull_is_rejected(self):
        before, draft, after = snapshots()
        after["pull"]["merged"] = False
        self.assertIn("not merged", " ".join(verify(before, draft, after)))

    def test_head_rewrite_is_rejected(self):
        before, draft, after = snapshots()
        after["pull"]["head"]["sha"] = "rewritten"
        self.assertIn("head changed", " ".join(verify(before, draft, after)))

    def test_approval_from_another_app_or_head_is_rejected(self):
        for field, value in (("app", {"id": 1}), ("head_sha", "other")):
            with self.subTest(field=field):
                before, draft, after = snapshots()
                before["head-checks"][0][field] = value
                self.assertIn("approval", " ".join(verify(before, draft, after)))

    def test_failed_draft_ci_is_rejected(self):
        before, draft, after = snapshots()
        draft["head-checks"][0]["conclusion"] = "failure"
        self.assertIn("draft CI", " ".join(verify(before, draft, after)))

    def test_original_cannot_substitute_for_temporary_draft(self):
        before, _, after = snapshots()
        self.assertIn("temporary draft", " ".join(verify(before, before, after)))

    def test_newer_failed_check_overrides_old_success(self):
        before, draft, after = snapshots()
        after["head-checks"].append({**after["head-checks"][-1], "id": 999999,
                                     "conclusion": "failure"})
        self.assertIn("queue merge", " ".join(verify(before, draft, after)))

    def test_advancement_requires_main_to_move_before_original_merges(self):
        before, draft, after = snapshots()
        advanced = copy.deepcopy(before)
        advanced["main"] = "main-advanced"
        self.assertEqual(verify(before, draft, after, advanced), [])
        for update, message in (({"main": "main-before"}, "main did not advance"),
                                ({"pull": after["pull"]}, "already merged")):
            changed = {**advanced, **update}
            self.assertIn(message, " ".join(verify(before, draft, after, changed)))

    def test_advancement_rejects_rewritten_or_substituted_original(self):
        before, draft, after = snapshots()
        for field, value in (("head", {"sha": "rewritten", "ref": "tl3/trial"}),
                             ("number", 999)):
            advanced = copy.deepcopy(before)
            advanced["main"] = "main-advanced"
            advanced["pull"][field] = value
            self.assertTrue(verify(before, draft, after, advanced))


if __name__ == "__main__":
    unittest.main()
