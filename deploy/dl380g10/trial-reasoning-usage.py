"""Issue #355 trial: what usage_metadata does the worker model path report with reasoning on?

Loads the model key in-process from the configured key_file. Prints usage and sizes only,
never the key, headers or response text.
"""
import json
from pathlib import Path
import sys
import tomllib

from langchain_core.messages import HumanMessage
from langchain_core.tools import tool

from tl3_workers import runtime
from tl3_workers.session import _message_characters

config = tomllib.loads(Path(sys.argv[1]).expanduser().read_text())["models"]
spec = config["maker"][1]  # attempt 2: claude-sonnet-5-5 at max effort
spec = {**spec, "base_url": config["base_url"], "budget_tokens": 16000}
request = {
    "key_file": config["key_file"],
    "model": spec,
    "operation": {"node": 355, "generation": 0},
    "inputs": {"attempt": 2},
    "role": "maker",
}


@tool
def read(path: str) -> str:
    """Read a file."""
    return ""


model = runtime.create_model(request).bind_tools([read], parallel_tool_calls=False)
prompts = {
    "tool_call": "Think carefully about whether 2**61-1 is prime and why, then call the read tool on path 'notes.txt'.",
    "text": "Think carefully, then answer in exactly one short sentence: is 2**61-1 prime?",
}
for name, prompt in prompts.items():
    final = None
    for chunk in model.stream([HumanMessage(content=prompt)]):
        final = chunk if final is None else final + chunk
    print(json.dumps({
        "case": name,
        "usage_metadata": final.usage_metadata,
        "visible_characters": _message_characters(final),
        "tool_calls": len(final.tool_calls),
        "additional_kwargs_keys": sorted(final.additional_kwargs),
        "reasoning_content_characters": len(str(final.additional_kwargs.get("reasoning_content", ""))),
        "response_metadata_keys": sorted(final.response_metadata),
    }), flush=True)
