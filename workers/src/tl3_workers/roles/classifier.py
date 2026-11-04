"""Classify one comment against the request and its open question."""

from tl3_workers.roles.base import Role
from tl3_workers.reply_check import check_reply as validate_reply


def check_reply(inputs, reply):
    validate_reply("classifier", inputs, reply)


ROLE = Role(
    name="classifier",
    prompt="classifier.md",
    input_schema="classifier-input",
    reply_schema="classifier-reply",
    tools={},
    check_reply=check_reply,
)
