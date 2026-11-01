"""Run one role with the TL3 Deep Agents profile."""

from collections.abc import Callable
from contextlib import nullcontext
from copy import deepcopy
from importlib.resources import files
import json
from pathlib import Path

from deepagents import create_deep_agent
from deepagents.middleware.filesystem import FilesystemMiddleware
from deepagents.profiles import (
    GeneralPurposeSubagentProfile,
    HarnessProfile,
    register_harness_profile,
)
from langchain_core.language_models import BaseChatModel
from langchain_core.messages import AIMessage, HumanMessage, ToolMessage
from langchain_openai import ChatOpenAI

from tl3_workers.contracts import read_json, schema, validate
from tl3_workers.roles import ROLES
from tl3_workers.tools import Worktree, build_tools
from tl3_workers.memory import Summary, current
from tl3_workers.exploration import ExplorationMiddleware
from tl3_workers.session import is_unaccepted_session


BUILTIN_TOOL_DESCRIPTIONS = {
    "ls": "Unavailable. Use only this role's TL3 tools.",
    "read_file": "Unavailable. Use only this role's TL3 tools.",
    "write_file": "Unavailable. Use only this role's TL3 tools.",
    "edit_file": "Unavailable. Use only this role's TL3 tools.",
    "delete": "Unavailable. Use only this role's TL3 tools.",
    "glob": "Unavailable unless supplied by TL3 for this role.",
    "grep": "Unavailable unless supplied by TL3 for this role.",
    "execute": "Execution is unavailable. Tests must use PrismaBuild.",
    "task": "Delegation is unavailable.",
}


class ProxyChatOpenAI(ChatOpenAI):
    def _get_ls_params(self, stop=None, **kwargs):
        params = super()._get_ls_params(stop=stop, **kwargs)
        # Built-in per-model profiles can inject prompts and tools. TL3 owns its profile.
        params["ls_provider"] = "tl3_proxy"
        return params


    def _stream(self, *args, **kwargs):
        seen = set()
        for chunk in super()._stream(*args, **kwargs):
            if chunk.message.usage_metadata is not None:
                usage = json.dumps(chunk.message.usage_metadata, sort_keys=True)
                # CLIProxyAPI repeats the same total on finish and trailing chunks.
                if usage in seen:
                    chunk = chunk.model_copy(update={"message": chunk.message.model_copy(
                        update={"usage_metadata": None})})
                else:
                    seen.add(usage)
            yield chunk


def session_identity(request):
    operation = request["operation"]
    generation = operation["generation"]
    if "session_file" in request and request["role"] in {"maker", "reviewer"}:
        path = Path(request["session_file"])
        if request["role"] == "maker":
            path = path.with_suffix(".head")
        if path.exists():
            saved = read_json(path.read_text())
            if is_unaccepted_session(request, saved):
                generation = saved["generation"]
    attempt = request["inputs"].get("attempt", request.get("trace", {}).get("attempt", 0))
    return f"tl3-{operation['node']}-{generation}-{attempt}"


def create_model(request) -> BaseChatModel:
    key = Path(request["key_file"]).read_text().strip()
    if not key:
        raise ValueError("empty model key")
    spec = request["model"]
    identity = session_identity(request)
    return ProxyChatOpenAI(
        base_url=spec["base_url"],
        model=spec["name"],
        reasoning_effort=spec["effort"],
        max_completion_tokens=spec["budget_tokens"],
        api_key=key,
        use_responses_api=False,
        streaming=True,
        stream_usage=True,
        default_headers={"X-Session-ID": identity},
        model_kwargs={"prompt_cache_key": identity},
        max_retries=0,
    )


def build_agent(model: BaseChatModel, role, worktree=None, *, reply_schema=None, context_size=150_000):
    descriptions = {**BUILTIN_TOOL_DESCRIPTIONS, **role.tools}
    profile = HarnessProfile(
        base_system_prompt="",
        system_prompt_suffix="",
        general_purpose_subagent=GeneralPurposeSubagentProfile(enabled=False),
        # Deep Agents unions exclusions across registrations. glob and grep are
        # TL3 tools, registered only for roles that declare them, never built-ins.
        excluded_tools=frozenset(BUILTIN_TOOL_DESCRIPTIONS.keys() - {"glob", "grep"}),
        excluded_middleware=frozenset({
            "SummarizationMiddleware", "PatchToolCallsMiddleware",
            "AnthropicPromptCachingMiddleware", "UnsupportedContentMiddleware",
        }),
        tool_description_overrides=descriptions,
    )
    provider = model._get_ls_params()["ls_provider"]
    identifier = getattr(model, "model_name", None) or getattr(model, "model", None)
    register_harness_profile(provider, profile)
    if identifier:
        register_harness_profile(f"{provider}:{identifier}", profile)
    prompt = files("tl3_workers").joinpath("prompts", role.prompt).read_text()
    prompt += "\n\nReply with one JSON object matching this schema:\n"
    proposal = deepcopy(schema(role.reply_schema)) if reply_schema is None else reply_schema
    for variant in proposal.get("oneOf", [proposal]):
        for name in role.code_only_fields:
            variant["properties"].pop(name, None)
    for name in role.code_only_fields:
        proposal.get("$defs", {}).pop(name, None)
    prompt += json.dumps(proposal, ensure_ascii=False)
    # Deep Agents requires read_file in its filesystem middleware. The profile
    # excludes it at both model and dispatch boundaries; StateBackend holds no host files.
    filesystem = FilesystemMiddleware(
        tools=["read_file"],
        system_prompt="",
        custom_tool_descriptions=descriptions,
        human_message_token_limit_before_evict=None,
        tool_token_limit_before_evict=None,
    )
    middleware = [filesystem]
    if role.exploration_budget is not None:
        middleware.append(ExplorationMiddleware(role.exploration_budget, role.name, context_size))
    return create_deep_agent(
        model=model, tools=build_tools(worktree, role.tools), subagents=[], system_prompt=prompt,
        middleware=middleware, name="tl3_" + role.name,
    )


def run(request, emit: Callable[[dict], None]):
    from tl3_workers.tracing import observe

    with observe(request):
        summary = Summary(request)
        token = current.set(summary)
        summary.save()
        reply, error = None, None
        try:
            reply = _run(request, emit)
        except BaseException as exc:
            error = exc
            raise
        finally:
            summary.finish(reply, error)
            current.reset(token)


def _run(request, emit):
    role = ROLES[request["role"]]
    model = create_model(request)
    with Worktree(request["worktree"]) if role.tools else nullcontext() as worktree:
        if worktree is not None:
            worktree.hindsight_key_file = request.get("hindsight_key_file")
            worktree.progress_emit = emit
        if role.run is None:
            agent = build_agent(model, role, worktree, context_size=request["inputs"].get(
                "context_size", request["inputs"].get("forecast", {}).get("context_tokens", 150_000)))
            reply = _run_agent(agent, request, emit)
        else:
            reply = role.run(model, request, worktree, emit)
    if not role.validates_reply:
        validate(role.reply_schema, reply)
        if role.check_reply is not None:
            role.check_reply(request["inputs"], reply)
    emit({"event": "reply", "role": role.name, "reply": reply})
    return reply


def _run_agent(agent, request, emit):
    last_message = None
    for update in agent.stream(
        {"messages": [HumanMessage(content=json.dumps(request["inputs"], ensure_ascii=False))]},
        stream_mode="updates",
    ):
        for result in update.values():
            if not isinstance(result, dict):
                continue
            for message in result.get("messages", []):
                if isinstance(message, AIMessage):
                    emit({"event": "progress", "unit": "model_turn"})
                    last_message = message
                elif isinstance(message, ToolMessage):
                    emit({"event": "progress", "unit": "tool_result"})
    if last_message is None or last_message.tool_calls or last_message.invalid_tool_calls:
        raise ValueError("missing final model reply")
    return read_json(last_message.text)
