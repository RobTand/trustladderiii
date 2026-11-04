"""Shared postmortem judge fixtures and worker-run helpers."""

import pytest

from tl3_workers.roles import postmortem


KEY = "tl3-planted-model-key-7c1e09"
SECRET = "password=assigned-password-403"
TOKEN = "sk-test403abcdefghijklmnopqrst"


RUBRIC = {"version": 1, "dimensions": {
    "reliability": {"grader": "code", "fact": "fix rounds"},
    "correctness": {"grader": "judge", "grades": {"5": "Right.", "1": "Wrong."}},
    "fix round": {"grader": "judge", "grades": {"5": "Fixed all.", "1": "Fixed nothing."}},
}}


MAKER_PROMPT = "workers/src/tl3_workers/prompts/maker.md"


class FakeTraces:
    def __init__(self, traces):
        self.traces, self.asked, self.logged = traces, [], []

    def find(self, tags, limit):
        self.asked.append(tags)
        return self.traces[:limit]

    def assessment(self, trace_id, key):
        for index, item in enumerate(self.logged, 1):
            if item["trace_id"] == trace_id and item["metadata"]["idempotency_key"] == key:
                return f"a-{index}"
        return None

    def log(self, trace_id, name, value, rationale, judge_model, metadata):
        self.logged.append({"trace_id": trace_id, "name": name, "value": value,
                            "rationale": rationale, "judge_model": judge_model,
                            "metadata": metadata})
        return f"a-{len(self.logged)}"


@pytest.fixture(autouse=True)
def judge_environment(monkeypatch):
    # judges.configure sets these for its process; the test restores them.
    for name, value in [("OPENAI_API_KEY", "placeholder"), ("MLFLOW_DISABLE_TELEMETRY", "true"),
                        ("DO_NOT_TRACK", "true"), ("MLFLOW_JUDGE_MAX_ITERATIONS", "30")]:
        monkeypatch.setenv(name, value)


@pytest.fixture(autouse=True)
def release(tmp_path, monkeypatch):
    """The files of the release the postmortem judges, as its recommendations target."""
    root = tmp_path / "release"
    prompt = root / MAKER_PROMPT
    prompt.parent.mkdir(parents=True)
    prompt.write_text("You are the maker.\nRun the tests.\nReply when done.\n")
    monkeypatch.setattr(postmortem, "RELEASE_ROOT", root)
    return root


def request(tmp_path, *, release="b" * 40):
    key_file = tmp_path / "model-key"
    key_file.write_text(KEY + "\n")
    worktree = tmp_path / "worktree"
    worktree.mkdir(exist_ok=True)
    case = {
        "request": {"repo": "RobTand/tl3-sandbox", "issue": 42, "title": "Fix the build",
                    "text": f"Fix it. The deploy key is {KEY}; {SECRET}; token {TOKEN}."},
        "outcome": "Merged", "grades": [{"dimension": "reliability", "grade": 4}],
        "phases": [{"phase": "Making", "ms": 90}],
        "questions": [{"kind": "cant_meet", "findings": {"decision": f"See {TOKEN}"}}],
        "incidents": [], "plan": {"pieces": []}, "brief": {"goals": [SECRET]},
        "audit": {"verdict": "agree"}, "release": release,
    }
    return {
        "protocol": 1, "operation": {"node": 3, "generation": 7, "kind": "postmortem"},
        "role": "postmortem",
        "model": {"base_url": "http://proxy.invalid:8317/v1", "name": "claude-opus-5-5",
                  "effort": "high", "budget_tokens": 32000},
        "key_file": str(key_file), "worktree": str(worktree),
        "inputs": {"record": {"id": 9, "trigger": "close", "reason": "needed_fix"},
                   "case": case, "rubric": RUBRIC, "release": release,
                   "grade_model": "claude-sonnet-5-5", "deadline_secs": 1200,
                   "trace_tags": {"node": "3", "request": "3", "repo": "RobTand/tl3-sandbox",
                                  "issue": "42"}},
    }


def run(document):
    from tl3_workers.runtime import run as run_worker

    events = []
    run_worker(document, events.append)
    [reply] = [event["reply"] for event in events if event["event"] == "reply"]
    return reply, events
