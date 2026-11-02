"""Deterministic prefix-cache estimate for offline worker replays.

This assumes every unchanged prefix token is cached, without provider minimums,
rounding or eviction. These costs are estimates, not live billing measurements.
"""
import math
import json

from tl3_workers.forecast import _encoding


class ReplayUsage:
    def __init__(self, cached):
        self.cached = cached
        self.encoding = _encoding()
        self.previous_tokens = []
        self.previous_messages = []

    def measure(self, messages, tools):
        serialized = [m.model_dump(include={"type", "content", "tool_calls", "tool_call_id", "name"})
                      for m in messages]
        payload = json.dumps({"messages": serialized, "tools": tools}, ensure_ascii=False)
        tokens = self.encoding.encode_ordinary(payload)
        common = 0
        for before, after in zip(self.previous_tokens, tokens):
            if before != after:
                break
            common += 1
        read = common if self.cached else 0
        creation = len(tokens) - read if self.cached else 0
        record = {"input_tokens": len(tokens), "cache_read": read, "cache_creation": creation,
                  "weighted_tokens": math.ceil((len(tokens) - read - creation) + creation * 1.25 + read * 0.1),
                  "input_bytes": len(payload.encode()),
                  "prefix_preserved": serialized[:len(self.previous_messages)] == self.previous_messages,
                  "tool_result_bytes": sum(len(m.text.encode()) for m in messages if m.type == "tool"),
                  "history_bytes": sum(len(json.dumps(m.model_dump(), ensure_ascii=False).encode())
                                       for m in messages if m.type in {"ai", "tool"})}
        self.previous_tokens = tokens
        self.previous_messages = serialized
        usage = {"input_tokens": len(tokens), "output_tokens": 1, "total_tokens": len(tokens) + 1}
        if self.cached:
            usage["input_token_details"] = {"cache_read": read, "cache_creation": creation}
        return record, usage
