"""Checkpoint persistence and context accounting shared by session roles."""

import json
import os

from langchain_core.callbacks import BaseCallbackHandler
from langchain_core.messages import AIMessage

from tl3_workers.budget_check import check as budget_check


def save(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(".tmp")
    with temporary.open("w") as handle:
        json.dump(value, handle, ensure_ascii=False, allow_nan=False)
        handle.flush()
        os.fsync(handle.fileno())
    os.replace(temporary, path)
    directory = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(directory)
    finally:
        os.close(directory)


def is_unaccepted_session(request, saved):
    # RunFailed advances the generation, but only an accepted reply ends a visit.
    return saved is not None and saved["generation"] > request.get("last_accepted_generation", -1)


def _message_characters(message):
    content = message.content
    count = len(content) if isinstance(content, str) else len(json.dumps(content, ensure_ascii=False))
    if isinstance(message, AIMessage) and message.tool_calls:
        count += len(json.dumps(message.tool_calls, ensure_ascii=False))
    return count


class ContextCharacters(BaseCallbackHandler):
    prompt = 0

    def on_chat_model_start(self, serialized, messages, **kwargs):
        # Count the actual prompt, including the schema, history and tool results.
        self.prompt = sum(_message_characters(message) for message in messages[0])
        tools = kwargs.get("invocation_params", {}).get("tools")
        if tools:
            self.prompt += len(json.dumps(tools, ensure_ascii=False))


def out_of_room(message, context_size, prompt_characters):
    usage = message.usage_metadata
    facts = dict(input=usage.get("input_tokens") if usage is not None else None,
                 output=usage.get("output_tokens") if usage is not None else None,
                 reasoning=(usage.get("output_token_details") or {}).get("reasoning") if usage else None,
                 visible_characters=None, fallback_characters=None,
                 prompt_characters=prompt_characters, context=context_size)
    while True:
        action = budget_check("cutoff_sample", **facts)
        if action["kind"] == "done":
            return action["out_of_room"]
        field = "visible_characters" if action["kind"] == "need_visible" else "fallback_characters"
        facts[field] = _message_characters(message)
