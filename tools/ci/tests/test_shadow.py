"""A batch agrees only when both gates reached the same verdict."""
import sys
from pathlib import Path
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import compare_shadow as shadow


class Comparison(unittest.TestCase):
    def report(self, old, new):
        pipeline = {"number": 7}
        run = {"head_sha": "a" * 40, "head_branch": "mergify/merge-queue/test", "html_url": "https://github.com/run", "run_started_at": "2026-10-31T12:00:00Z", "updated_at": "2026-10-31T12:02:00Z", "status": "completed"}
        workflows = [{"id": "workflow", "status": "success", "created_at": "2026-10-31T12:00:00Z", "stopped_at": "2026-10-31T12:01:00Z"}]
        jobs = [{"name": name, "conclusion": status} for name, status in old.items()]
        circle_jobs = [{"name": f"CircleCI {name}", "status": status} for name, status in new.items()]
        return shadow.compare(pipeline, run, workflows, jobs, circle_jobs)

    def test_passing_checks_agree(self):
        result = self.report({"Rust": "success", "Python": "success"}, {"Rust": "success", "Python": "success"})
        self.assertIs(result["agreement"], True)
        self.assertEqual(result["github_seconds"], 120)
        self.assertEqual(result["circleci_workflows"][0]["seconds"], 60)

    def test_opposite_failures_do_not_agree(self):
        result = self.report({"Rust": "failure", "Python": "success"}, {"Rust": "success", "Python": "failed"})
        self.assertIs(result["agreement"], False)

    def test_missing_or_canceled_checks_are_not_evidence(self):
        for new in ({"Rust": "success"}, {"Rust": "success", "Python": "canceled"}):
            with self.subTest(new=new):
                self.assertIsNone(self.report({"Rust": "success", "Python": "success"}, new)["agreement"])

    def test_matching_failures_agree(self):
        self.assertIs(self.report({"Rust": "failure", "Python": "success"}, {"Rust": "failed", "Python": "success"})["agreement"], True)


if __name__ == "__main__":
    unittest.main()
