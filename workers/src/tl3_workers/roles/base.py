"""A role's resources and capabilities, independent of the agent framework."""

from collections.abc import Callable, Mapping
from dataclasses import dataclass, field


@dataclass(frozen=True)
class Role:
    name: str
    prompt: str
    input_schema: str
    reply_schema: str
    tools: Mapping[str, str] = field(default_factory=dict)
    check_reply: Callable[[dict, dict], None] | None = None
    run: Callable[[object, dict, object, Callable[[dict], None]], dict] | None = None
