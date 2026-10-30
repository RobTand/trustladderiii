"""Write a leaf's code, stop before context exhaustion, and commit in worker code."""

import json
import os
from pathlib import Path
import subprocess

from langchain_core.messages import AIMessage, HumanMessage, ToolMessage
from langchain_openai import ChatOpenAI

from tl3_workers.contracts import read_json, validate
from tl3_workers.pb_tests import TestSettings
from tl3_workers.roles.base import Role
from tl3_workers.tools import _parts


def _check_inputs(inputs):
    goals = [goal["id"] for goal in inputs["goals"]]
    if len(set(goals)) != len(goals):
        raise ValueError("goal ids must be unique")
    for path in inputs["plan"]["files"]:
        if not _parts(path):
            raise ValueError("expected a declared file")
    for item in [*inputs["plan"]["checks"], *inputs.get("feedback", [])]:
        if not set(item["goal_ids"]) <= set(goals):
            raise ValueError("unknown goal id")
    if {goal for check in inputs["plan"]["checks"] for goal in check["goal_ids"]} != set(goals):
        raise ValueError("checks must cover every goal")


def _check_reply(inputs, reply):
    if reply["kind"] == "done":
        goals = [goal["id"] for goal in inputs["goals"]]
        checked = [check["goal_id"] for check in reply["self_check"]]
        if len(checked) != len(goals) or set(checked) != set(goals):
            raise ValueError("self-check must cover every goal exactly once")
    elif reply["kind"] == "needs_files":
        for item in reply["files"]:
            if not _parts(item["path"]):
                raise ValueError("expected a requested file")


def _commit(request):
    # Discard inherited Git overrides as well as user/system configuration.
    env = {
        "PATH": os.defpath, "LANG": "C.UTF-8",
        "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_SYSTEM": os.devnull,
        "GIT_CONFIG_GLOBAL": os.devnull,
        "GIT_AUTHOR_NAME": "TL3", "GIT_AUTHOR_EMAIL": "tl3@example.invalid",
        "GIT_COMMITTER_NAME": "TL3", "GIT_COMMITTER_EMAIL": "tl3@example.invalid",
    }

    def git(*args, check=True):
        return subprocess.run(
            ["git", "-c", "core.hooksPath=/dev/null", "-c", "commit.gpgsign=false",
             "-c", "core.fsmonitor=false", *args],
            cwd=request["worktree"], env=env, check=check,
            stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True,
        )

    root = git("rev-parse", "--show-toplevel").stdout.strip()
    if Path(root).resolve() != Path(request["worktree"]).resolve():
        raise ValueError("maker requires the worktree root")
    before = git("rev-parse", "--verify", "HEAD").stdout.strip()
    git("add", "--all", "--", ".")
    diff = git("diff", "--cached", "--quiet", "--no-ext-diff", "HEAD", "--", check=False)
    if diff.returncode == 0:
        raise ValueError("done requires code changes")
    if diff.returncode != 1:
        raise ValueError("could not inspect staged changes")
    inputs = request["inputs"]
    git("commit", "--quiet", "--no-gpg-sign", "-m",
        f"TL3 maker attempt {inputs['attempt']}, round {inputs['round']}")
    head = git("rev-parse", "--verify", "HEAD").stdout.strip()
    if head == before:
        raise ValueError("done did not create a commit")
    return head


def _out_of_room(message, context_size):
    usage = message.usage_metadata
    if usage is None:
        raise ValueError("maker requires model usage metadata")
    counts = [usage.get("input_tokens"), usage.get("output_tokens")]
    if any(type(count) is not int or count < 0 for count in counts):
        raise ValueError("invalid model usage metadata")
    # Each prompt resends the conversation. Summing turn totals double-counts it.
    return sum(counts) * 100 >= context_size * 85


def run(model, request, worktree, emit):
    from tl3_workers.runtime import build_agent

    inputs = request["inputs"]
    _check_inputs(inputs)
    worktree.test_settings = TestSettings(**inputs["tests"])
    if isinstance(model, ChatOpenAI):
        model.stream_usage = True
        model.model_kwargs = {**model.model_kwargs, "parallel_tool_calls": False}
    agent = build_agent(model, ROLE, worktree)
    last_message = None
    stream = agent.stream(
        {"messages": [HumanMessage(content=json.dumps(inputs, ensure_ascii=False))]},
        stream_mode="updates",
    )
    try:
        for update in stream:
            for result in update.values():
                if not isinstance(result, dict):
                    continue
                for message in result.get("messages", []):
                    if isinstance(message, AIMessage):
                        emit({"event": "progress", "unit": "model_turn"})
                        if _out_of_room(message, inputs["context_size"]):
                            return {"kind": "out_of_room"}
                        last_message = message
                    elif isinstance(message, ToolMessage):
                        emit({"event": "progress", "unit": "tool_result"})
    finally:
        stream.close()
    if last_message is None or last_message.tool_calls or last_message.invalid_tool_calls:
        raise ValueError("missing final maker reply")
    reply = read_json(last_message.text)
    validate("maker-reply", reply)
    _check_reply(inputs, reply)
    if reply["kind"] == "done":
        if "head" in reply:
            raise ValueError("the worker supplies the commit head")
        reply["head"] = _commit(request)
    return reply


ROLE = Role(
    name="maker",
    prompt="maker.md",
    input_schema="maker-input",
    reply_schema="maker-reply",
    tools={
        "list": "List worktree files needed to implement the declared plan.",
        "read": "Read worktree text to implement or self-check a goal.",
        "glob": "Find relative worktree paths relevant to the declared plan.",
        "grep": "Find literal references in worktree text while implementing a goal.",
        "write": "Create or overwrite a declared file with UTF-8 text.",
        "edit": "Replace exact text in a declared file. Read it first and use a unique match.",
        "run_tests": "Run named pytest tests or the suite through PrismaBuild. No shell or commands.",
    },
    run=run,
)
