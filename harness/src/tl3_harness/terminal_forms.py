"""Shared terminal declarations; the sibling rules core uses only the standard library."""

from .rules import TerminalForm, register_terminal


def _cant_meet(reply):
    return isinstance(reply, dict) and reply.get('kind') == 'cant_meet'


PLANNER = register_terminal(TerminalForm('planner', _cant_meet, 'Return a reply with kind "cant_meet".'))
MAKER = register_terminal(TerminalForm('maker', _cant_meet, 'Return a reply with kind "cant_meet".'))
