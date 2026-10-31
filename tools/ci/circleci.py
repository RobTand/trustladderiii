"""CircleCI test selection and execution. Development checks run through PrismaBuild."""
from pathlib import Path


def rust_filter(changed, packages, full=False):
    return "all()"


def python_groups(files, projects):
    return {}
