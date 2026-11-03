"""Plan one change and measure its context with a model-agnostic tokenizer.

Deployment must preload tiktoken's o200k_base data in TIKTOKEN_CACHE_DIR.
"""

import base64
import codecs
import hashlib
import os
from pathlib import Path, PurePosixPath
import sys

import tiktoken

from tl3_workers.tools import LARGE_FILE_BYTES, Worktree, _parts, environment_class
from tl3_workers.turns import Unrepairable


# A declared file larger than LARGE_FILE_BYTES is read by range and edited by
# unique string replacement, so it costs a bounded allowance, not its full size (#321).
RANGED_FILE_TOKENS = 16_384
# Larger declared files can't be used at all.
HARD_CAP_BYTES = 16 * 1_048_576
# Notes in the brief name at most this many files each.
NOTE_PATHS = 10
TOKENIZER_URL = "https://openaipublic.blob.core.windows.net/encodings/o200k_base.tiktoken"
TOKENIZER_SHA256 = "446a9538cb6c348e3516120d7c08b09f57c36495e2acfffe59a5bf8b0cfb1a2d"


def _fail(message):
    # The process boundary hides exception text, which may contain secrets.
    # These fixed diagnostics contain no repository or provider content.
    print(f"tl3_workers: planner: {message}", file=sys.stderr)
    raise Unrepairable(message)


def _encoding():
    encoding = optional_encoding()
    if encoding is None:
        _fail("o200k_base tokenizer data must be preloaded in TIKTOKEN_CACHE_DIR")
    return encoding


def optional_encoding():
    """Load the pinned tokenizer, or silently return None if its cache is unavailable."""
    cache = os.environ.get("TIKTOKEN_CACHE_DIR")
    if not cache:
        return None
    key = hashlib.sha1(TOKENIZER_URL.encode()).hexdigest()
    try:
        with (Path(cache) / key).open("rb") as stream:
            data = stream.read(4 * 1024 * 1024)
    except OSError:
        return None
    if hashlib.sha256(data).hexdigest() != TOKENIZER_SHA256:
        return None
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


class DeclaredFileError(ValueError):
    """A declared file the forecast can't use, named with its measured size.

    It's the model's reply problem, not a crash: a retry would fail the same way
    (#321). Callers report it in the reply. #314's repair turn can show
    `str(error)` to the model instead.
    """

    def __init__(self, path, reason, size=None):
        self.path, self.reason, self.size = path, reason, size
        super().__init__(f"{path}: {reason}" + (f" ({size} bytes)" if size is not None else ""))


def _environment(error):
    # Q50: an environment failure is infrastructure, not the model's file problem.
    return isinstance(error, OSError) and environment_class(error) is not None


def _file_tokens(worktree, path, encoding):
    """Return the file's token cost, and its size when it is read by range.
    Without an encoding, a small file costs its byte count, an upper bound."""
    try:
        parts = _parts(path)
    except ValueError as error:
        raise DeclaredFileError(path, str(error)) from None
    if not parts or str(PurePosixPath(path)) != path:
        raise DeclaredFileError(path, "not a canonical relative path")
    # New files, including those in new directories, have no base content.
    # Existing ancestors still pass the file tools' descriptor confinement.
    try:
        with worktree._directory(parts[:-1]) as parent:
            os.stat(parts[-1], dir_fd=parent, follow_symlinks=False)
    except FileNotFoundError:
        return 0, None
    except (OSError, ValueError) as error:
        if _environment(error):
            raise
        raise DeclaredFileError(path, "not a regular file inside the worktree") from None
    try:
        with worktree._file(path) as stream:
            size = os.fstat(stream.fileno()).st_size
            if size > HARD_CAP_BYTES:
                raise DeclaredFileError(path, f"over the {HARD_CAP_BYTES}-byte limit", size)
            decoder = codecs.getincrementaldecoder("utf-8")()
            chunks = []
            read = 0
            while chunk := stream.buffer.read(1_048_576):
                read += len(chunk)
                if read > HARD_CAP_BYTES:
                    raise DeclaredFileError(path, f"over the {HARD_CAP_BYTES}-byte limit", read)
                if any(byte < 32 and byte not in (9, 10, 13) for byte in chunk):
                    raise DeclaredFileError(path, "binary", size)
                text = decoder.decode(chunk)
                if read <= LARGE_FILE_BYTES:
                    chunks.append(text)
            decoder.decode(b"", final=True)
    except UnicodeDecodeError:
        raise DeclaredFileError(path, "not UTF-8 text", size) from None
    except (OSError, ValueError) as error:
        if isinstance(error, DeclaredFileError) or _environment(error):
            raise
        raise DeclaredFileError(path, "not a regular file inside the worktree") from None
    if read > LARGE_FILE_BYTES:
        return RANGED_FILE_TOKENS, read
    if encoding is None:
        return read, None
    return len(encoding.encode_ordinary("".join(chunks))), None


def measure_files(worktree, paths, encoding=None):
    """Cost declared files with one rule for every role: a large file costs a
    ranged-edit allowance and an unusable one is a reply problem (#321).
    Returns the costs, the problems and the large files' sizes. Environment
    errors propagate as infrastructure failures (Q50)."""
    file_tokens, problems, large = {}, [], {}
    for path in sorted(set(paths)):
        try:
            file_tokens[path], size = _file_tokens(worktree, path, encoding)
        except DeclaredFileError as error:
            problems.append(error)
            file_tokens[path] = 0
            continue
        if size is not None:
            large[path] = size
    return file_tokens, problems, large


def forecast_fit(brief, file_tokens, config, encoding=None, problems=()):
    """Use the same context budget for planning and requesting more files.

    Configured overhead reserves tools, making history and fix-round feedback.
    Without cached tokenizer data, UTF-8 bytes conservatively bound brief tokens.
    Unusable files each cost the entire fit limit, so they cannot prove a fit.
    """
    brief_tokens = len(encoding.encode_ordinary(brief) if encoding is not None else brief.encode())
    overhead_tokens = sum(config["overhead_tokens"].values())
    tokens = brief_tokens + sum(file_tokens.values()) + overhead_tokens + config["fit_tokens"] * len(problems)
    return {
        "tokenizer": "o200k_base", "brief_tokens": brief_tokens,
        "file_tokens": file_tokens, "overhead_tokens": overhead_tokens,
        "tokens": tokens, "fit_tokens": config["fit_tokens"],
        "context_tokens": config["context_tokens"], "fits": tokens < config["fit_tokens"],
    }


class Forecaster:
    def __init__(self, config):
        if config["fit_tokens"] >= config["context_tokens"]:
            raise ValueError("fit limit must be smaller than context size")
        self.config = config
        self.encoding = _encoding()
        self.problems, self.large = [], {}

    def files(self, paths):
        """Measure declared files. Unusable files cost nothing and are kept in
        `problems`; large files are kept in `large` with their sizes."""
        with Worktree(".") as worktree:
            file_tokens, self.problems, self.large = measure_files(worktree, paths, self.encoding)
        return file_tokens

    def forecast(self, brief, file_tokens):
        return forecast_fit(brief, file_tokens, self.config, self.encoding)

    def measure(self, brief, paths):
        return self.forecast(brief, self.files(paths))

    def notes(self):
        """Guidance for the maker about the files last measured, for the brief.
        Each note names at most NOTE_PATHS files."""
        def listed(items):
            items = list(items)
            more = f", and {len(items) - NOTE_PATHS} more" if len(items) > NOTE_PATHS else ""
            return "; ".join(items[:NOTE_PATHS]) + more

        notes = []
        if self.large:
            notes.append(
                "These declared files are too large to read whole. Read them by range with "
                "read's offset and limit, or find lines with grep first, and change them with "
                "edit's unique exact-text replacement: "
                + listed(f"{path} ({size} bytes)" for path, size in sorted(self.large.items()))
                + ".")
        if self.problems:
            notes.append("TL3's file tools can't use these declared files: "
                         + listed(str(error) for error in self.problems) + ".")
        return notes
