"""Worker membership adapter for the stdlib-only harness refusal rows."""

from jsonschema import Draft202012Validator

from tl3_harness.rules import (
    Allowed as MembershipAllowed, CantMeet, Edit, Facts, Name, Rule, Violation,
    apply_edit, at, evaluate, permits, register, render_all,
)

__all__ = [
    'Allowed', 'CantMeet', 'Edit', 'Facts', 'Name', 'Rule', 'Violation',
    'apply_edit', 'at', 'evaluate', 'permits', 'register', 'render_all',
]


class Allowed(MembershipAllowed):
    """Read membership from the same JSON Schema constants as reply validation."""

    def accepts(self, value):
        from tl3_workers.contracts import registry
        constraint = self.value if self.keyword == 'schema' else {self.keyword: self.value}
        return Draft202012Validator(constraint, registry=registry()).is_valid(value)
