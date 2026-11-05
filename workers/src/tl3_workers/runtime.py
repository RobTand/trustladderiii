"""Run one role with the TL3 Deep Agents profile."""

from collections.abc import Callable
from contextlib import nullcontext
from contextvars import ContextVar
from copy import deepcopy
from hashlib import sha256
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
from jsonschema import ValidationError

from tl3_workers.evidence import forcing_capabilities, forcing_guidance
from tl3_workers.contracts import model_facing, read_json, read_reply, schema, validate
from tl3_workers.roles import ROLES
from tl3_workers.roles.planner import EXPLORATION_DECLARATION, PlanConsistencyError, planner_schema_reply
from tl3_workers.forcing_rehearsal import ForcingRehearsals
from tl3_workers.tools import Worktree, build_tools
from tl3_workers.memory import Summary, current
from tl3_workers.recall import redacted_memories
from tl3_workers.exploration import ExplorationMiddleware
from tl3_workers.turns import TurnControl, repaired
from tl3_workers.session import is_unaccepted_session
from tl3_workers.prompt_render import render as render_prompt, selection as prompt_renderer


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
    def _get_request_payload(self, *args, **kwargs):
        payload = super()._get_request_payload(*args, **kwargs)
        messages = payload["messages"]
        if self.model_name.startswith("claude") and any(
            isinstance(message.get("content"), list) and any(
                isinstance(block, dict) and "cache_control" in block for block in message["content"])
            for message in messages
        ):
            # CLIProxyAPI v8.0.51 skips automatic markers when any explicit marker exists.
            # Its translator hoists this onto the final text/tool-result block.
            # Mark only this payload so earlier rolling markers never enter saved history.
            payload["messages"] = [*messages[:-1], {**messages[-1], "cache_control": {"type": "ephemeral"}}]
        return payload

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
        try:
            for chunk in super()._stream(*args, **kwargs):
                if chunk.message.usage_metadata is not None:
                    # CLIProxyAPI sends totals on finish and again on a trailing chunk.
                    last_usage = chunk.message.usage_metadata
                    chunk = chunk.model_copy(update={"message": chunk.message.model_copy(
                        update={"usage_metadata": None})})
                yield chunk
        finally:
            # Keep measured usage even when the stream fails after its usage chunk.
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
    from tl3_workers.facts import current as current_facts
    if facts := current_facts.get():
        facts.session_id = identity
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
        model_kwargs={"prompt_cache_key": prompt_cache_identity(request)},
        max_retries=0,
    )


ONLY_JSON = "Reply with one JSON object only: no prose, no code fences."
# The run's prompt version from the controller (PRD #327, C5), if it sent one.
run_prompt = ContextVar("run_prompt", default=None)
run_test_cpus = ContextVar("run_test_cpus", default=1)


def role_prompt(role, *, text=None, test_cpus=None):
    supplied = run_prompt.get() if text is None else text
    if rendered := render_prompt('role', {'role': role.name, 'text': supplied,
                                        'test_cpus': run_test_cpus.get() if test_cpus is None else test_cpus}):
        return rendered['text']
    prompt = supplied if supplied is not None else files("tl3_workers").joinpath("prompts", role.prompt).read_text()
    if role.name in {"ingest", "audit", "planner"}:
        prompt += "\n\n" + forcing_guidance(run_test_cpus.get() if test_cpus is None else test_cpus)
    if role.name == 'planner':
        from tl3_workers.forcing_rehearsal import GUIDANCE
        prompt += "\n\n" + GUIDANCE
    return prompt


def shared_inputs(inputs):
    # Intake has bounded orientation; planning has the immutable base SHA instead.
    return {name: inputs[name] for name in ("repo", "base_sha") if name in inputs}


def prompt_cache_identity(request):
    role = ROLES[request["role"]]
    if role.name not in {"ingest", "audit", "planner"}:
        return session_identity(request)
    prompt = request.get("prompt", {})
    supplied = prompt.get('text')
    if supplied is None:
        supplied = run_prompt.get()
    if rendered := render_prompt('cache', {**request, 'prompt': {**prompt, 'text': supplied}}):
        return rendered['identity']
    shared = {
        "role": role.name,
        "version": prompt.get("version"),
        "prompt": role_prompt(role, text=prompt.get("text"), test_cpus=request.get("test_cpus", 1)),
        "schema": schema(role.reply_schema),
        "planner_schema": EXPLORATION_DECLARATION if role.name == "planner" else None,
        "format": ONLY_JSON,
        "repository": request.get("trace", {}).get("repo", request.get("memory", {}).get("repo")),
        "orientation": shared_inputs(request["inputs"]),
    }
    digest = sha256(json.dumps(shared, sort_keys=True, ensure_ascii=False).encode()).hexdigest()
    return "tl3-cache-" + digest[:48]


def initial_message(request, inputs):
    rendered = render_prompt('prefix', {**request, 'inputs': inputs})
    if rendered is not None:
        prefix = rendered['prefix']
        if prefix is None:
            return HumanMessage(content=json.dumps(inputs, ensure_ascii=False))
        prefix = {'type': prefix['type'], 'text': prefix['text'],
                  **{name: value for name, value in prefix.items() if name not in {'type', 'text'}}}
        specific = {name: value for name, value in inputs.items() if name not in {'repo', 'base_sha'}}
        return HumanMessage(content=[prefix, {
            'type': 'text', 'text': json.dumps(specific, ensure_ascii=False)[1:],
        }])
    shared = shared_inputs(inputs) if request["role"] in {"ingest", "audit", "planner"} else {}
    if not shared:
        return HumanMessage(content=json.dumps(inputs, ensure_ascii=False))
    specific = {name: value for name, value in inputs.items() if name not in shared}
    # Two text blocks retain one JSON object and the orientation's user priority.
    # CLIProxyAPI preserves Claude cache_control on ChatOpenAI text blocks.
    prefix = {"type": "text", "text": json.dumps(shared, sort_keys=True, ensure_ascii=False)[:-1] + ", "}
    if request.get("model", {}).get("name", "").startswith("claude"):
        prefix["cache_control"] = {"type": "ephemeral"}
    return HumanMessage(content=[prefix, {
        "type": "text", "text": json.dumps(specific, ensure_ascii=False)[1:],
    }])


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
    prompt = role_prompt(role)
    if worktree is not None and role.name in {"ingest", "planner", "maker"}:
        prompt += "\n\n" + worktree.generation_instruction()
    prompt += "\n\n" + ONLY_JSON + " It must match this schema:\n"
    proposal = deepcopy(schema(role.reply_schema)) if reply_schema is None else reply_schema
    if role.name == 'planner':
        proposal = {**EXPLORATION_DECLARATION, **proposal}
    for variant in proposal.get("oneOf", [proposal]):
        for name in role.code_only_fields:
            variant["properties"].pop(name, None)
    for name in role.code_only_fields:
        proposal.get("$defs", {}).pop(name, None)
    prompt += json.dumps(model_facing(proposal), ensure_ascii=False)
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
    control.hidden_tools = profile.excluded_tools
    if role.exploration_budget is not None:
        # The allowance wraps the turn control, so it charges dropped calls and repairs.
        control.allowance = ExplorationMiddleware(role.exploration_budget, role.name, context_size)
        worktree.allowance = control.allowance
        middleware.append(control.allowance)
    middleware.append(control)
    return create_deep_agent(
        model=model, tools=build_tools(worktree, role.tools), subagents=[], system_prompt=prompt,
        middleware=middleware, name="tl3_" + role.name,
    )


def run(request, emit: Callable[[dict], None]):
    from tl3_workers.tracing import observe, record_reply

    from tl3_workers.facts import observe as observe_facts

    with observe_facts(request), observe(request):
        summary = Summary(request)
        token = current.set(summary)
        prompt = run_prompt.set(request.get("prompt", {}).get("text"))
        test_cpus = run_test_cpus.set(request.get("test_cpus", 1))
        renderer = prompt_renderer.set({})
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
            prompt_renderer.reset(renderer)
            run_prompt.reset(prompt)
            run_test_cpus.reset(test_cpus)
            current.reset(token)


def _run(request, emit):
    role = ROLES[request["role"]]
    model = create_model(request)
    if request["inputs"].get("memories"):
        request = {**request, "inputs": {**request["inputs"],
                                         "memories": redacted_memories(request["inputs"]["memories"])}}
    if role.prepare_inputs is not None:
        request = {**request, "inputs": role.prepare_inputs(request["inputs"], request)}
    forecast = request["inputs"].get("forecast", {})
    with Worktree(request["worktree"], generated_files=forecast.get("generated_files", ()),
                  generator=forecast.get("generator")) if role.tools else nullcontext() as worktree:
        if worktree is not None:
            worktree.environment_errors_end_run = role.environment_errors_end_run
            worktree.hindsight_key_file = request.get("hindsight_key_file")
            worktree.progress_emit = emit
        if role.name == 'planner':
            worktree.forcing_rehearsals = ForcingRehearsals(
                worktree, request['inputs'], checkout=request['worktree'], checkpoint=request.get('session_file'))
        if role.run is None:
            control = TurnControl()
            agent = build_agent(model, role, worktree, context_size=request["inputs"].get(
                "context_size", request["inputs"].get("forecast", {}).get("context_tokens", 150_000)),
                control=control)
            model_inputs = request["inputs"]
            if role.name in {"ingest", "audit", "planner"}:
                model_inputs = {"forcing_capabilities": forcing_capabilities(run_test_cpus.get()), **model_inputs}
            if role.name == "planner":
                maker_budget = ROLES["maker"].exploration_budget
                model_inputs = {**model_inputs, "maker_budget": {
                    "tool_turns": maker_budget.tool_turns, "input_tokens": maker_budget.input_tokens,
                    "finalization_turns": maker_budget.finalization_turns,
                    "no_progress_window": maker_budget.no_progress_window}}
            ledger = getattr(worktree, 'forcing_rehearsals', None)
            if ledger is not None and (ledger.candidates or ledger.active):
                model_inputs = {**model_inputs, 'forcing_rehearsal_state': {
                    'candidates': ledger.candidates, 'pending': ledger.active,
                    'remaining_candidates': 8 - ledger.attempts}}
            history = [initial_message(request, model_inputs)]
            draft_error = None

            def checked(text):
                nonlocal draft_error
                if ledger is not None:
                    ledger.raise_infrastructure()
                if ledger is not None and ledger.failure:
                    return _checked(role, request['inputs'], {
                        'kind': 'cant_meet', 'files': [], 'evidence_contract': [], 'report': ledger.failure['message']}, worktree=worktree)
                try:
                    result = _checked(role, request["inputs"], read_reply(text), worktree=worktree)
                except PlanConsistencyError as error:
                    if ledger is not None and any(item['status'] == 'refused' for item in ledger.candidates):
                        return refused_final()
                    if draft_error is None:
                        draft_error = error
                    raise
                except (ValueError, ValidationError):
                    if ledger is not None and any(item['status'] == 'refused' for item in ledger.candidates):
                        return refused_final()
                    raise
                if draft_error is not None:
                    draft_error.constrain(result)
                return result

            def refused_final():
                rejected = next(item for item in reversed(ledger.candidates) if item['status'] == 'refused')
                return _checked(role, request['inputs'], {
                    'kind': 'cant_meet', 'files': [], 'evidence_contract': [],
                    'report': rejected['message']}, worktree=worktree)

            reply = repaired(control, history, lambda: _run_agent(agent, history, emit),
                             checked)
        else:
            reply = role.run(model, request, worktree, emit)
            if not role.validates_reply:
                _checked(role, request["inputs"], reply)
    emit({"event": "reply", "role": role.name, "reply": reply})
    return reply


def _checked(role, inputs, reply, *, worktree=None):
    validate(role.reply_schema, planner_schema_reply(reply) if role.name == 'planner' else reply)
    if role.check_reply is not None:
        if role.name == 'planner':
            role.check_reply(inputs, reply, rehearsals=getattr(worktree, 'forcing_rehearsals', None))
        else:
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
