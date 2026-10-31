"""Pytest function selectors shared by collection and report validation."""


def selects(requested: str, collected: str) -> bool:
    return (requested == collected or
            "[" not in requested and collected.startswith(requested + "[")
            and collected.endswith("]"))
