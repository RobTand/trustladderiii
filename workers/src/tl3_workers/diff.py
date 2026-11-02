"""A read-only, paged view of the worktree's change against a base commit (#434).

Scope goals ("touches only ...") need the change, and the model has no Git. The tool runs
fixed Git read commands in a hardened environment: no model input reaches an option, and
nothing it runs writes the repository's index, objects, refs or files. It writes one thing,
a throwaway copy of the index in a temporary directory, so that untracked files appear as
additions in a single `git diff` stream.

Its cost is bounded by structure, not by a clock: a page runs a fixed number of Git
processes whatever the number of files, a line is read through a bounded buffer, and files
that are not regular or are over DIFF_FILE_BYTES are skipped and named.
"""

from functools import cache
import json
import os
from pathlib import Path
import re
import shutil
import stat
import subprocess
import tempfile

# Read-only commands only. Repository configuration must not run programs or change output:
# fsmonitor and a hooks path would execute, as would an external or text-conversion diff
# driver and a filter driver. Filter drivers come from configuration the tool cannot
# override wholesale, so each one is cleared by name (see _Git).
_ENV = {
    "PATH": os.defpath, "LANG": "C.UTF-8", "GIT_OPTIONAL_LOCKS": "0", "GIT_TERMINAL_PROMPT": "0",
    "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_SYSTEM": os.devnull, "GIT_CONFIG_GLOBAL": os.devnull,
}
_GIT = ["git", "--literal-pathspecs", "-c", "core.hooksPath=/dev/null", "-c", "core.fsmonitor=false",
        "-c", "core.quotePath=false", "-c", "diff.renames=false", "-c", "core.pager=cat",
        "-c", "core.attributesFile=/dev/null"]
_DIFF = ["diff", "--no-ext-diff", "--no-textconv", "--no-color", "--no-renames"]
_FILTER_KEYS = "^filter\\..*\\.(clean|smudge|process)$"
STAT_ENTRIES = 100
UNTRACKED_ENTRIES = 200
STAT_BYTES = 6_144
SKIPPED_NAMES = 5
# The planner's declared-file hard cap (forecast.HARD_CAP_BYTES; the test keeps them equal).
DIFF_FILE_BYTES = 16 * 1_048_576
_SHA = re.compile(r"[0-9a-f]{40}|[0-9a-f]{64}")
# A line longer than the cap is discarded in chunks of this size, never held whole.
_DISCARD = 65_536


@cache
def _version():
    done = subprocess.run(["git", "--version"], env=_ENV, stdout=subprocess.PIPE, check=False)
    found = re.search(rb"(\d+)\.(\d+)", done.stdout)
    return (int(found[1]), int(found[2])) if found else (0, 0)


class _Git:
    """Git for one checkout, with every filter driver the repository configures cleared."""

    def __init__(self, checkout, limits):
        self.checkout, self.limits = Path(checkout), limits
        self.prefix = list(_GIT)
        configured = self.run("config", "-z", "--name-only", "--get-regexp", _FILTER_KEYS, ok=(0, 1))
        names = {key[len("filter."):key.rindex(".")] for key in configured.split("\0") if key}
        if any("=" in name for name in names):
            # `-c name=value` cannot carry a name with "=", so the driver could not be cleared.
            raise ValueError("diff is unavailable: the repository defines a filter driver "
                             "whose name Git cannot be told to disable.")
        for name in sorted(names):
            for variable in ("clean", "smudge", "process"):
                self.prefix += ["-c", f"filter.{name}.{variable}="]
            self.prefix += ["-c", f"filter.{name}.required=false"]
        if _version() >= (2, 40):
            # Worktree and index .gitattributes then no longer apply. $GIT_DIR/info/attributes
            # still does, which is why the drivers are cleared above as well.
            empty = self.run("hash-object", "-t", "tree", "--stdin", stdin=b"").strip()
            self.prefix.insert(1, f"--attr-source={empty}")

    def run(self, *args, env=None, ok=(0,), stdin=None):
        done = subprocess.run([*self.prefix, *args], cwd=self.checkout, env={**_ENV, **(env or {})},
                              input=stdin, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, check=False)
        if done.returncode not in ok:
            raise ValueError("diff could not read the change from Git.")
        return done.stdout.decode("utf-8", errors="replace")

    def lines(self, *args, env=None):
        """Stream a command's lines, each cut at line_bytes, and stop the command if the reader stops."""
        cap = self.limits["line_bytes"]
        process = subprocess.Popen([*self.prefix, *args], cwd=self.checkout, env={**_ENV, **(env or {})},
                                   stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
        try:
            while raw := process.stdout.readline(cap + 1):
                if len(raw) > cap and not raw.endswith(b"\n"):
                    while (rest := process.stdout.readline(_DISCARD)) and not rest.endswith(b"\n"):
                        pass
                    yield raw[:cap].decode("utf-8", errors="ignore") + f" [line cut at {cap} bytes]\n"
                else:
                    text = raw.decode("utf-8", errors="replace")
                    yield text if text.endswith("\n") else text + "\n"
        finally:
            process.kill()
            process.stdout.close()
            process.wait()


def _split(output):
    return [item for item in output.split("\0") if item]


def _diffable(checkout, path):
    """Whether Git may diff this worktree path: a regular file or link, or gone, and not huge."""
    try:
        info = os.lstat(checkout / path)
    except (FileNotFoundError, NotADirectoryError):
        return True
    except OSError:
        return False
    if stat.S_ISLNK(info.st_mode):
        return True
    return stat.S_ISREG(info.st_mode) and info.st_size <= DIFF_FILE_BYTES


def _numstat(output):
    for item in _split(output):
        added, deleted, path = item.split("\t", 2)
        yield path, None if added == "-" else int(added), None if deleted == "-" else int(deleted)


def make_diff_tool(worktree, parts, limits):
    def diff(path: str = ".", offset: int = 0) -> dict:
        """Show the worktree's change against its base as a stat and unified hunks, paged by line. Offset continues a truncated page."""
        if offset < 0:
            raise ValueError("Offset must be nonnegative")
        spec = "/".join(parts(path)) or "."
        checkout, base = getattr(worktree, "diff_checkout", None), getattr(worktree, "diff_base", None)
        if checkout is None:
            raise ValueError("diff is not available in this run.")
        checkout = Path(checkout)
        git = _Git(checkout, limits)
        base = base or "HEAD"
        commit = "" if base != "HEAD" and not _SHA.fullmatch(base) else git.run(
            "rev-parse", "--verify", "--quiet", f"{base}^{{commit}}", ok=(0, 1)).strip()
        if not commit:
            raise ValueError("The base commit is not in this worktree.")
        tracked = _split(git.run(*_DIFF, "--name-only", "-z", commit, "--", spec))
        untracked = _split(git.run("ls-files", "--others", "--exclude-standard", "-z", "--", spec))
        skipped = [name for name in [*tracked, *untracked] if not _diffable(checkout, name)]
        untracked = [name for name in untracked if name not in skipped]
        shown = untracked[:UNTRACKED_ENTRIES]
        notes = []
        if len(untracked) > len(shown):
            notes.append(f"{len(untracked) - len(shown)} more untracked files are not shown; narrow the path.")
        if skipped:
            many = len(skipped) > 1
            names = ", ".join(skipped[:SKIPPED_NAMES]) + (", ..." if len(skipped) > SKIPPED_NAMES else "")
            notes.append(f"{len(skipped)} changed {'files were' if many else 'file was'} skipped because "
                         f"{'they are' if many else 'it is'} not regular files or "
                         f"{'are' if many else 'is'} over {DIFF_FILE_BYTES:,} bytes: {names}.")
        # Skipped tracked files are left out by naming the rest, since a pathspec cannot exclude
        # under --literal-pathspecs.
        keep = [name for name in tracked if name not in skipped]
        scope = [spec] if len(keep) == len(tracked) else keep
        with tempfile.TemporaryDirectory() as scratch:
            index = Path(scratch) / "index"
            source = Path(checkout, git.run("rev-parse", "--git-path", "index").strip())
            if source.is_file():
                shutil.copyfile(source, index)
            env = {"GIT_INDEX_FILE": str(index)}
            if shown:
                # Intent-to-add records the path with the empty blob and writes no object, so
                # untracked files diff as additions in the same stream as the tracked changes.
                git.run("add", "--intent-to-add", "--", *shown, env=env)
                scope = [spec] if scope == [spec] else [*scope, *shown]
            if not scope:
                return {"base": commit[:12], "stat": [], "diff": "", "next_offset": None,
                        "message": " ".join(["No changes against the base.", *notes])}
            result = {"base": commit[:12]}
            if offset == 0:
                status = _split(git.run(*_DIFF, "--name-status", "-z", commit, "--", *scope, env=env))
                codes = dict(zip(status[1::2], status[0::2]))
                counted = sorted(_numstat(git.run(*_DIFF, "--numstat", "-z", commit, "--", *scope, env=env)))
                if not counted:
                    where = "against the base" if path == "." else f"under {path}"
                    return {**result, "stat": [], "diff": "", "next_offset": None,
                            "message": " ".join([f"No changes {where}.", *notes])}
                entries = [{"path": name, "status": codes.get(name, "M"), "added": added, "deleted": deleted}
                           for name, added, deleted in counted]
                listed = []
                for entry in entries[:STAT_ENTRIES]:
                    if len(json.dumps([*listed, entry]).encode()) > STAT_BYTES:
                        break
                    listed.append(entry)
                result["stat"] = listed
                if len(entries) > len(listed):
                    notes.append(f"{len(entries) - len(listed)} more changed files are not listed; narrow the path.")
            else:
                notes = []
            budget = limits["bytes"] - 768 - len(json.dumps(result).encode())
            pieces, used, taken, more = [], 0, 0, False
            stream = git.lines(*_DIFF, commit, "--", *scope, env=env)
            for number, line in enumerate(stream):
                if number < offset:
                    continue
                size = len(json.dumps(line, ensure_ascii=True).encode())
                if taken == limits["read_lines"] or used + size > budget:
                    more = True
                    break
                pieces.append(line)
                used += size
                taken += 1
            stream.close()
        result["diff"] = "".join(pieces)
        result["next_offset"] = offset + taken if more else None
        if more:
            notes.insert(0, f"Truncated. Continue with offset={offset + taken}.")
        result["message"] = " ".join(notes)
        return result

    return diff
