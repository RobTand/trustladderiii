"""Plan one change and measure its context with a model-agnostic tokenizer.

Deployment must preload tiktoken's o200k_base data in TIKTOKEN_CACHE_DIR.
"""

import base64
import hashlib
import os
from pathlib import Path, PurePosixPath
import sys

import tiktoken

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


def measure(brief, paths, config):
    if config["fit_tokens"] >= config["context_tokens"]:
        raise ValueError("fit limit must be smaller than context size")
    encoding = _encoding()
    brief_tokens = len(encoding.encode_ordinary(brief))
    with Worktree(".") as worktree:
        file_tokens = {path: _file_tokens(worktree, path, encoding) for path in sorted(set(paths))}
    overhead_tokens = sum(config["overhead_tokens"].values())
    tokens = brief_tokens + sum(file_tokens.values()) + overhead_tokens
    return {
        "tokenizer": "o200k_base", "brief_tokens": brief_tokens,
        "file_tokens": file_tokens, "overhead_tokens": overhead_tokens,
        "tokens": tokens, "fit_tokens": config["fit_tokens"],
        "context_tokens": config["context_tokens"], "fits": tokens < config["fit_tokens"],
    }
