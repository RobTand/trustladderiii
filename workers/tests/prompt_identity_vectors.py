"""Record the real worker prompt outputs before the Rust migration (#819).

Run with the workers environment, with --check to refuse drift. No model or
tool executes. Capture maker's run-time preparation and the postmortem judge's
contract; other roles use build_agent's actual system prompt.
"""
import argparse
from copy import deepcopy
from hashlib import sha256
import json
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

from tl3_workers import judges, runtime
from tl3_workers.roles import maker


VECTORS = Path(__file__).with_name("prompt-identity-vectors.json")
SCHEMA_MARKER = " It must match this schema:\n"


def digest(text):
    data = text.encode("utf-8")
    return {"sha256": sha256(data).hexdigest(), "utf8_bytes": len(data)}


class CapturedPrompt(Exception):
    """Stop maker immediately after its real schema reaches build_agent."""


def schema_text(role):
    if role.name == "postmortem":
        return judges.postmortem_contract("").split("Authoritative result JSON Schema:\n", 1)[1]
    model = SimpleNamespace(_get_ls_params=lambda: {"ls_provider": "tl3_fixture"})
    worktree = SimpleNamespace(generation_instruction=lambda: "Fixture generation instruction.")
    build_agent = runtime.build_agent

    def capture(*args, **kwargs):
        raise CapturedPrompt(build_agent(*args, **kwargs))

    with patch.object(runtime, "build_tools", return_value=[]), patch.object(
            runtime, "create_deep_agent", side_effect=lambda **kwargs: kwargs["system_prompt"]):
        if role.name == "maker":
            request = {"worktree": "/fixture/worktree", "inputs": {
                "goals": [], "plan": {"files": [], "checks": []}, "context_size": 150_000,
                "tests": {"image": "sha256:" + "a" * 64, "tools_path": "/fixture/tools"},
            }}
            with patch.object(runtime, "build_agent", side_effect=capture):
                try:
                    maker.run(model, request, worktree, lambda event: None)
                except CapturedPrompt as captured:
                    system = captured.args[0]
                else:
                    raise AssertionError("maker did not build its agent")
        else:
            system = build_agent(model, role, worktree)
    return system.split(SCHEMA_MARKER, 1)[1]


def requests(role):
    # The orientation matches worker_process.request_doc and shared_request.
    orientation = {"readme": "A greeting library.", "file_list": ["greeting.py"]}
    for cpus in ([1, 4] if role.name in {"ingest", "audit", "planner"} else [1]):
        for supplied in (False, True):
            request = {
                "role": role.name,
                "operation": {"node": 17, "generation": 3, "kind": role.name},
                "trace": {"repo": "owner/served", "attempt": 2},
                "test_cpus": cpus,
                "inputs": {"repo": deepcopy(orientation), "base_sha": "a" * 40,
                           "task": "Preserve café and greeting.", "attempt": 2},
            }
            if supplied:
                request["prompt"] = {"version": 7, "text": "Fixture prompt: preserve café."}
            yield request


def role_vectors(role):
    identities = []
    for request in requests(role):
        prompt = request.get("prompt", {})
        identities.append({
            "request": request,
            "prompt_cache_identity": runtime.prompt_cache_identity(request),
            "session_identity": runtime.session_identity(request),
            "role_prompt": digest(runtime.role_prompt(
                role, text=prompt.get("text"), test_cpus=request["test_cpus"])),
        })
    messages = []
    for shared in (False, True):
        for model in ("claude-sonnet-5-5", "gpt-6.1-sol"):
            request = {"role": role.name, "model": {"name": model}}
            inputs = {"task": "Preserve café and greeting."}
            if shared:
                inputs = {"repo": {"readme": "A greeting library.", "file_list": ["greeting.py"]},
                          "base_sha": "a" * 40, **inputs}
            messages.append({"request": request, "inputs": inputs,
                             "content": runtime.initial_message(request, inputs).content})
    return {"model_facing_schema": digest(schema_text(role)),
            "identities": identities, "initial_messages": messages}


def generate():
    return {"format": "tl3.prompt-identity.v1",
            "roles": {name: role_vectors(role) for name, role in runtime.ROLES.items()}}


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true")
    parser.add_argument("--output", type=Path, default=VECTORS)
    args = parser.parse_args(argv)
    text = json.dumps(generate(), indent=2, ensure_ascii=False) + "\n"
    if args.check:
        if args.output.read_text(encoding="utf-8") != text:
            parser.exit(1, "Prompt identity vectors drifted; investigate before regenerating (#819).\n")
    else:
        args.output.write_text(text, encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
