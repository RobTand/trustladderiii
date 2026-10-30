"""Plan one change and measure its context with a model-agnostic tokenizer.

Deployment must preload tiktoken's o200k_base data in TIKTOKEN_CACHE_DIR.
"""

import base64
import hashlib
import os
from pathlib import Path, PurePosixPath
import sys

import tiktoken

from tl3_workers.roles.base import Role
from tl3_workers.tools import Worktree, _parts


MAX_FILE_BYTES = 1_048_576
TOKENIZER_URL = "https://openaipublic.blob.core.windows.net/encodings/o200k_base.tiktoken"
TOKENIZER_SHA256 = "446a9538cb6c348e3516120d7c08b09f57c36495e2acfffe59a5bf8b0cfb1a2d"


def _fail(message):
    # The process boundary hides exception text, which may contain secrets.
    # These fixed diagnostics contain no repository or provider content.
    print(f"tl3_workers: planner: {message}", file=sys.stderr)
    raise ValueError(message)


def _encoding():
    message = "o200k_base tokenizer data must be preloaded in TIKTOKEN_CACHE_DIR"
    cache = os.environ.get("TIKTOKEN_CACHE_DIR")
    if not cache:
        _fail(message)
    key = hashlib.sha1(TOKENIZER_URL.encode()).hexdigest()
    try:
        with (Path(cache) / key).open("rb") as stream:
            data = stream.read(4 * 1024 * 1024)
    except OSError:
        _fail(message)
    if hashlib.sha256(data).hexdigest() != TOKENIZER_SHA256:
        _fail(message)
    ranks = {
        base64.b64decode(token): int(rank)
        for token, rank in (line.split() for line in data.splitlines() if line)
    }
    # o200k_base's definition from pinned tiktoken 0.14.0. Construct locally:
    # get_encoding's loader can download again if the cache changes after a check.
    return tiktoken.Encoding(
        name="o200k_base",
        pat_str="|".join([
            r"[^\r\n\p{L}\p{N}]?[\p{Lu}\p{Lt}\p{Lm}\p{Lo}\p{M}]*[\p{Ll}\p{Lm}\p{Lo}\p{M}]+(?i:'s|'t|'re|'ve|'m|'ll|'d)?",
            r"[^\r\n\p{L}\p{N}]?[\p{Lu}\p{Lt}\p{Lm}\p{Lo}\p{M}]+[\p{Ll}\p{Lm}\p{Lo}\p{M}]*(?i:'s|'t|'re|'ve|'m|'ll|'d)?",
            r"\p{N}{1,3}",
            r" ?[^\s\p{L}\p{N}]+[\r\n/]*",
            r"\s*[\r\n]+",
            r"\s+(?!\S)",
            r"\s+",
        ]),
        mergeable_ranks=ranks,
        special_tokens={"<|endoftext|>": 199999, "<|endofprompt|>": 200018},
    )


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
        if os.fstat(stream.fileno()).st_size > MAX_FILE_BYTES:
            _fail("declared file exceeds the 1048576-byte limit")
        data = stream.buffer.read(MAX_FILE_BYTES + 1)
    if len(data) > MAX_FILE_BYTES:
        _fail("declared file exceeds the 1048576-byte limit")
    message = "declared file must be UTF-8 text without binary control bytes"
    if any(byte < 32 and byte not in (9, 10, 13) for byte in data):
        _fail(message)
    try:
        content = data.decode("utf-8")
    except UnicodeDecodeError:
        _fail(message)
    return len(encoding.encode_ordinary(content))


def check_reply(inputs, reply):
    if "forecast" in reply:
        raise ValueError("only code may supply a forecast")
    config = inputs["forecast"]
    if config["fit_tokens"] >= config["context_tokens"]:
        raise ValueError("fit limit must be smaller than context size")
    goals = set(range(len(inputs["ingest"]["goals"])))
    paths = set()
    if reply["kind"] == "leaf":
        if inputs["must_split"]:
            _fail("must split after running out of room")
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

    if reply["kind"] in {"leaf", "split"}:
        for path in paths:
            if any(PurePosixPath(path).full_match(pattern) for pattern in inputs["protected_paths"]):
                _fail("declared file matches a protected path")

    encoding = _encoding()
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
