"""Plan one change and measure its context with a model-agnostic tokenizer.

Deployment must cache tiktoken's o200k_base data before restricting network
access. Its locked version already ships through langchain-openai.
"""

import os
from pathlib import PurePosixPath

import tiktoken

from tl3_workers.roles.base import Role
from tl3_workers.tools import Worktree, _parts


def _file_tokens(worktree, path, encoding):
    parts = _parts(path)
    if not parts or str(PurePosixPath(path)) != path:
        raise ValueError("declared files must use canonical relative paths")
    # New files, including those in new directories, have no base content.
    # Existing ancestors still pass the file tools' descriptor confinement.
    try:
        with worktree._directory(parts[:-1]) as parent:
            os.stat(parts[-1], dir_fd=parent, follow_symlinks=False)
    except FileNotFoundError:
        return 0
    with worktree._file(path) as stream:
        return len(encoding.encode_ordinary(stream.read()))


def check_reply(inputs, reply):
    if "forecast" in reply:
        raise ValueError("only code may supply a forecast")
    config = inputs["forecast"]
    if config["fit_tokens"] >= config["context_tokens"]:
        raise ValueError("fit limit must be smaller than context size")
    goals = set(range(len(inputs["ingest"]["goals"])))
    paths = set()
    if reply["kind"] == "leaf":
        covered = set()
        for check in reply["checks"]:
            for node_id in check["node_ids"]:
                path, *names = node_id.split("::")
                _parts(path)
                if not path.endswith(".py") or not names or not all(names) or path.startswith("-"):
                    raise ValueError("checks must name exact pytest node IDs")
            cited = set(check["goal_ids"])
            if not cited <= goals:
                raise ValueError("check cites an unknown goal")
            covered.update(cited)
        if covered != goals:
            raise ValueError("every goal must have a check")
        paths.update(reply["files"])
    elif reply["kind"] == "split":
        covered = set()
        for piece in reply["pieces"]:
            cited = set(piece["goal_ids"])
            if not cited <= goals:
                raise ValueError("piece cites an unknown goal")
            covered.update(cited)
            paths.update(piece["files"])
        if covered != goals:
            raise ValueError("split must preserve every goal")
    else:
        paths.update(reply["files"])

    encoding = tiktoken.get_encoding("o200k_base")
    brief_tokens = len(encoding.encode_ordinary(inputs["ingest"]["brief"]))
    with Worktree(".") as worktree:
        file_tokens = {path: _file_tokens(worktree, path, encoding) for path in sorted(paths)}
    overhead_tokens = sum(config["overhead_tokens"].values())
    tokens = brief_tokens + sum(file_tokens.values()) + overhead_tokens
    # The shared runtime invokes this check after validating the model's shape
    # and before emitting its single reply. Code alone appends the measurement.
    reply["forecast"] = {
        "tokenizer": "o200k_base",
        "brief_tokens": brief_tokens,
        "file_tokens": file_tokens,
        "overhead_tokens": overhead_tokens,
        "tokens": tokens,
        "fit_tokens": config["fit_tokens"],
        "context_tokens": config["context_tokens"],
        "fits": tokens < config["fit_tokens"],
    }


ROLE = Role(
    name="planner",
    prompt="planner.md",
    input_schema="planner-input",
    reply_schema="planner-reply",
    tools={
        "list": "List worktree directories to locate material for the plan.",
        "read": "Read worktree text to identify files and checks for the goals.",
        "glob": "Find worktree paths using relative glob patterns for planning.",
        "grep": "Find literal references in worktree text to check the plan.",
    },
    check_reply=check_reply,
)
