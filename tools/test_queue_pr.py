import copy
import unittest
from unittest.mock import patch

import queue_pr


HEAD = "a" * 40
PR = {
    "head": {"sha": HEAD, "repo": {"full_name": queue_pr.REPO}},
    "base": {"ref": "main", "repo": {"full_name": queue_pr.REPO}},
    "state": "open", "draft": False, "merged": False,
    "mergeable": True, "mergeable_state": "blocked",
}


class QueueTests(unittest.TestCase):
    def invoke(self, pr, expected=HEAD, no_checks=False):
        calls = []

        def api(endpoint, *args):
            calls.append((endpoint, args))
            if endpoint.endswith("/pulls/42"):
                return pr
            if endpoint.endswith("/check-runs"):
                if no_checks:
                    self.fail("queue admission must not request PR-level CI")
                return {"check_runs": [
                    {"name": name, "app": {"slug": "github-actions"},
                     "started_at": "2026-10-31T00:00:00Z", "status": "completed",
                     "conclusion": "success"}
                    for name in ("Rust", "Python")
                ]}
            if endpoint.endswith("/comments"):
                return {"html_url": "https://github.com/example/comment"}
            self.fail(f"unexpected REST call: {endpoint}")

        with patch.object(queue_pr, "api", api):
            queue_pr.main("42", expected)
        return calls

    def test_reviewed_head_queues_without_pr_ci(self):
        calls = self.invoke(copy.deepcopy(PR), no_checks=True)
        self.assertEqual(calls, [
            (f"repos/{queue_pr.REPO}/pulls/42", ()),
            (f"repos/{queue_pr.REPO}/issues/42/comments", ("-f", "body=@mergifyio queue")),
        ])

    def test_rejects_partial_or_changed_reviewed_head(self):
        for expected in ("", "a", HEAD[:12], "b" * 40):
            with self.subTest(expected=expected), self.assertRaises(SystemExit):
                self.invoke(copy.deepcopy(PR), expected)

    def test_rejects_closed_draft_conflicting_and_unknown_mergeability(self):
        for changes in (
            {"state": "closed"}, {"draft": True}, {"merged": True},
            {"mergeable": False}, {"mergeable": None}, {"mergeable_state": "dirty"},
        ):
            with self.subTest(changes=changes), self.assertRaises(SystemExit):
                self.invoke({**copy.deepcopy(PR), **changes})

    def test_rejects_other_base_or_repository(self):
        for base in (
            {"ref": "release", "repo": {"full_name": queue_pr.REPO}},
            {"ref": "main", "repo": {"full_name": "RobTand/other"}},
        ):
            with self.subTest(base=base), self.assertRaises(SystemExit):
                self.invoke({**copy.deepcopy(PR), "base": base})


if __name__ == "__main__":
    unittest.main()
