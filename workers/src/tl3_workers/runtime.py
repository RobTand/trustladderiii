"""Run one role with the TL3 Deep Agents profile."""

from collections.abc import Callable
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


TOOL_DESCRIPTIONS = {
    "ls": "List files supplied to this role. Unavailable for ingest and audit.",
    "read_file": "Read material supplied to this role. Unavailable for ingest and audit.",
    "write_file": "Write within the assigned worktree. Unavailable for ingest and audit.",
    "edit_file": "Edit within the assigned worktree. Unavailable for ingest and audit.",
    "delete": "Remove a worktree file. Unavailable for ingest and audit.",
    "glob": "Find worktree paths. Unavailable for ingest and audit.",
    "grep": "Search worktree text. Unavailable for ingest and audit.",
    "execute": "Execution is unavailable. Tests must use PrismaBuild.",
    "task": "Delegation is unavailable for ingest and audit.",
}


class ProxyChatOpenAI(ChatOpenAI):
    def _get_ls_params(self, stop=None, **kwargs):
        params = super()._get_ls_params(stop=stop, **kwargs)
        # Built-in per-model profiles can inject prompts and tools. TL3 owns its profile.
        params["ls_provider"] = "tl3_proxy"
        return params


def create_model(request) -> BaseChatModel:
    key = Path(request["key_file"]).read_text().strip()
    if not key:
        raise ValueError("empty model key")
    spec = request["model"]
    return ProxyChatOpenAI(
        base_url=spec["base_url"],
        model=spec["name"],
        reasoning_effort=spec["effort"],
        max_completion_tokens=spec["budget_tokens"],
        api_key=key,
        use_responses_api=False,
        streaming=True,
        max_retries=0,
    )


def build_agent(model: BaseChatModel, role: str):
    profile = HarnessProfile(
        base_system_prompt="",
        system_prompt_suffix="",
        general_purpose_subagent=GeneralPurposeSubagentProfile(enabled=False),
        excluded_tools=frozenset(TOOL_DESCRIPTIONS),
        excluded_middleware=frozenset({
            "SummarizationMiddleware", "PatchToolCallsMiddleware",
            "AnthropicPromptCachingMiddleware", "UnsupportedContentMiddleware",
        }),
        tool_description_overrides=TOOL_DESCRIPTIONS,
    )
    provider = model._get_ls_params()["ls_provider"]
    identifier = getattr(model, "model_name", None) or getattr(model, "model", None)
    register_harness_profile(provider, profile)
    if identifier:
        register_harness_profile(f"{provider}:{identifier}", profile)
    prompt = files("tl3_workers").joinpath("prompts", role + ".md").read_text()
    prompt += "\n\nReply with one JSON object matching this schema:\n"
    prompt += json.dumps(schema(role + "-reply"), ensure_ascii=False)
    # Deep Agents requires read_file in its filesystem middleware. The profile
    # excludes it at both model and dispatch boundaries; StateBackend holds no host files.
    filesystem = FilesystemMiddleware(
        tools=["read_file"],
        system_prompt="",
        custom_tool_descriptions=TOOL_DESCRIPTIONS,
        human_message_token_limit_before_evict=None,
        tool_token_limit_before_evict=None,
    )
    return create_deep_agent(
        model=model, tools=[], subagents=[], system_prompt=prompt,
        middleware=[filesystem], name="tl3_" + role,
    )


def run(request, emit: Callable[[dict], None]):
    role = request["role"]
    model = create_model(request)
    agent = build_agent(model, role)
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
    reply = read_json(last_message.text)
    validate(role + "-reply", reply)
    if role == "ingest" and reply["kind"] == "brief":
        supplied = {memory["id"] for memory in request["inputs"].get("memories", [])}
        if not set(reply["memory_ids"]) <= supplied:
            raise ValueError("reply cites a memory it was not given")
    emit({"event": "reply", "role": role, "reply": reply})
