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
from langchain_core.messages import AIMessage, AIMessageChunk, HumanMessage, ToolMessage
from langchain_core.outputs import ChatGenerationChunk
from langchain_openai import ChatOpenAI

from tl3_workers.contracts import read_json, read_reply, schema, validate
from tl3_workers.roles import ROLES
from tl3_workers.tools import Worktree, build_tools
from tl3_workers.memory import Summary, current
from tl3_workers.exploration import ExplorationMiddleware
from tl3_workers.turns import TurnControl, repaired
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
    def _convert_chunk_to_generation_chunk(self, chunk, default_chunk_class, base_generation_info):
        from tl3_workers.tracing import record_chunk_usage
        record_chunk_usage(chunk)
        return super()._convert_chunk_to_generation_chunk(
            chunk, default_chunk_class, base_generation_info)

    def _get_ls_params(self, stop=None, **kwargs):
        params = super()._get_ls_params(stop=stop, **kwargs)
        # Built-in per-model profiles can inject prompts and tools. TL3 owns its profile.
        params["ls_provider"] = "tl3_proxy"
        return params

    def _stream(self, *args, **kwargs):
        last_usage = None
        for chunk in super()._stream(*args, **kwargs):
            if chunk.message.usage_metadata is not None:
                # CLIProxyAPI sends totals on finish and again on a trailing chunk.
                last_usage = chunk.message.usage_metadata
                chunk = chunk.model_copy(update={"message": chunk.message.model_copy(
                    update={"usage_metadata": None})})
            yield chunk
        if last_usage is not None:
            yield ChatGenerationChunk(message=AIMessageChunk(content="", usage_metadata=last_usage))


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
    from tl3_workers.tracing import record_model_key
    record_model_key(key)
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


ONLY_JSON = "Reply with one JSON object only: no prose, no code fences."


def build_agent(model: BaseChatModel, role, worktree=None, *, reply_schema=None, context_size=150_000,
                control=None):
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
    prompt += "\n\n" + ONLY_JSON + " It must match this schema:\n"
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
    control = TurnControl() if control is None else control
    if role.exploration_budget is not None:
        # The allowance wraps the turn control, so it charges dropped calls and repairs.
        control.allowance = ExplorationMiddleware(role.exploration_budget, role.name, context_size)
        middleware.append(control.allowance)
    middleware.append(control)
    return create_deep_agent(
        model=model, tools=build_tools(worktree, role.tools), subagents=[], system_prompt=prompt,
        middleware=middleware, name="tl3_" + role.name,
    )


def run(request, emit: Callable[[dict], None]):
    from tl3_workers.tracing import observe, record_reply

    with observe(request):
        summary = Summary(request)
        token = current.set(summary)
        summary.save()
        reply, error = None, None
        try:
            reply = _run(request, emit)
            record_reply(reply)
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
            control = TurnControl()
            agent = build_agent(model, role, worktree, context_size=request["inputs"].get(
                "context_size", request["inputs"].get("forecast", {}).get("context_tokens", 150_000)),
                control=control)
            history = [HumanMessage(content=json.dumps(request["inputs"], ensure_ascii=False))]
            reply = repaired(control, history, lambda: _run_agent(agent, history, emit),
                             lambda text: _checked(role, request["inputs"], read_reply(text)))
        else:
            reply = role.run(model, request, worktree, emit)
            if not role.validates_reply:
                _checked(role, request["inputs"], reply)
    emit({"event": "reply", "role": role.name, "reply": reply})
    return reply


def _checked(role, inputs, reply):
    validate(role.reply_schema, reply)
    if role.check_reply is not None:
        role.check_reply(inputs, reply)
    return reply


def _run_agent(agent, history, emit):
    last_message = None
    for update in agent.stream({"messages": list(history)}, stream_mode="updates"):
        for result in update.values():
            if not isinstance(result, dict):
                continue
            for message in result.get("messages", []):
                history.append(message)
                if isinstance(message, AIMessage):
                    emit({"event": "progress", "unit": "model_turn"})
                    last_message = message
                elif isinstance(message, ToolMessage):
                    emit({"event": "progress", "unit": "tool_result"})
    if last_message is None or last_message.tool_calls or last_message.invalid_tool_calls:
        raise ValueError("missing final model reply")
    return last_message.text
