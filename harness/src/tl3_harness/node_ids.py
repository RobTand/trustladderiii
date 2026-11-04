"""Pytest function selectors shared by collection and report validation."""


MAX_ALWAYS_TESTS = 19


def selects(requested: str, collected: str) -> bool:
    return (requested == collected or
            "::" not in requested and collected.startswith(requested + "::") or
            "[" not in requested and collected.startswith(requested + "[")
            and collected.endswith("]"))


def validate_always_tests(tests):
    """Validate trusted repo settings at the harness boundary; parameter labels are opaque."""
    if not isinstance(tests, (tuple, list)):
        raise ValueError("always_tests must list repo-relative pytest files or node IDs")
    if len(tests) > MAX_ALWAYS_TESTS:
        raise ValueError(f"always_tests accepts at most {MAX_ALWAYS_TESTS} entries")
    for test in tests:
        if not isinstance(test, str):
            raise ValueError("always_tests must contain strings")
        path, *names = test.split("::")
        if (not path.endswith(".py") or path.startswith(("/", "-"))
                or any(char in path for char in "\\[]")
                or any(part in ("", ".", "..") for part in path.split("/"))
                or not all(names) or any(ord(char) < 32 or 127 <= ord(char) <= 159 for char in test)):
            raise ValueError(f"not a repo-relative pytest file or node ID: {test!r}")
