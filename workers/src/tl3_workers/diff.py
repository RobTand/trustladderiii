"""A read-only, paged view of the worktree's change against a base commit (#434).

Scope goals ("touches only ...") need the change, and the model has no Git. The tool runs
fixed Git read commands in a hardened environment: no model input reaches an option, and
nothing it runs writes the index, the object database, a ref or the worktree.
"""

import json
import os
from pathlib import Path
import re
import subprocess

# Read-only commands only. Repository configuration must not run programs or change output:
# fsmonitor and a hooks path would execute, and an external or text-conversion diff driver would too.
_ENV = {
    "PATH": os.defpath, "LANG": "C.UTF-8", "GIT_OPTIONAL_LOCKS": "0", "GIT_TERMINAL_PROMPT": "0",
    "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_SYSTEM": os.devnull, "GIT_CONFIG_GLOBAL": os.devnull,
}
_GIT = ["git", "--literal-pathspecs", "-c", "core.hooksPath=/dev/null", "-c", "core.fsmonitor=false",
        "-c", "core.quotePath=false", "-c", "diff.renames=false", "-c", "core.pager=cat"]
_DIFF = ["diff", "--no-ext-diff", "--no-textconv", "--no-color", "--no-renames"]
STAT_ENTRIES = 100
UNTRACKED_ENTRIES = 200
STAT_BYTES = 6_144
# The planner's declared-file hard cap (forecast.HARD_CAP_BYTES; the test keeps them equal).
DIFF_FILE_BYTES = 16 * 1_048_576
_SHA = re.compile(r"[0-9a-f]{40}|[0-9a-f]{64}")


def _run(checkout, *args, ok=(0,)):
    done = subprocess.run([*_GIT, *args], cwd=checkout, env=_ENV, stdout=subprocess.PIPE,
                          stderr=subprocess.DEVNULL, check=False)
    if done.returncode not in ok:
        raise ValueError("diff could not read the change from Git.")
    return done.stdout.decode("utf-8", errors="replace")


def _lines(checkout, *args):
    """Stream a command's output lines, and stop the command if the reader stops."""
    process = subprocess.Popen([*_GIT, *args], cwd=checkout, env=_ENV, stdout=subprocess.PIPE,
                               stderr=subprocess.DEVNULL)
    try:
        for raw in process.stdout:
            yield raw.decode("utf-8", errors="replace")
    finally:
        process.kill()
        process.stdout.close()
        process.wait()


def _counts(field):
    return None if field == "-" else int(field)


def _numstat(output):
    for record in output.split("\0"):
        if record:
            added, deleted, path = record.split("\t", 2)
            yield path, _counts(added), _counts(deleted)


def _untracked(checkout, spec):
    listed = _run(checkout, "ls-files", "--others", "--exclude-standard", "-z", "--", spec)
    return [path for path in listed.split("\0") if path]


def _stat(checkout, base, spec, untracked):
    status = _run(checkout, *_DIFF, "--name-status", "-z", base, "--", spec).split("\0")
    codes = {path: code for code, path in zip(status[0::2], status[1::2])}
    entries = {}
    for path, added, deleted in _numstat(_run(checkout, *_DIFF, "--numstat", "-z", base, "--", spec)):
        entries[path] = {"path": path, "status": codes.get(path, "M"), "added": added, "deleted": deleted}
    for path in untracked[:UNTRACKED_ENTRIES]:
        counted = _run(checkout, *_DIFF, "--no-index", "--numstat", "-z", "--", os.devnull, path, ok=(0, 1))
        _, added, deleted = next(_numstat(counted), (path, None, None))
        entries[path] = {"path": path, "status": "A", "added": added, "deleted": deleted}
    return sorted(entries.values(), key=lambda entry: entry["path"])


def _hunks(checkout, base, spec, untracked):
    yield from _lines(checkout, *_DIFF, base, "--", spec)
    for path in untracked:
        yield from _lines(checkout, *_DIFF, "--no-index", "--", os.devnull, path)


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
        base = base or "HEAD"
        commit = "" if base != "HEAD" and not _SHA.fullmatch(base) else _run(
            checkout, "rev-parse", "--verify", "--quiet", f"{base}^{{commit}}", ok=(0, 1)).strip()
        if not commit:
            raise ValueError("The base commit is not in this worktree.")
        untracked = _untracked(checkout, spec)
        result = {"base": commit[:12]}
        extra = []
        if offset == 0:
            stat = _stat(checkout, commit, spec, untracked)
            extra = untracked[UNTRACKED_ENTRIES:]
            if not stat:
                where = "against the base" if path == "." else f"under {path}"
                return {**result, "stat": [], "diff": "", "next_offset": None, "message": f"No changes {where}."}
            listed = []
            for entry in stat[:STAT_ENTRIES]:
                if len(json.dumps([*listed, entry]).encode()) > STAT_BYTES:
                    break
                listed.append(entry)
            result["stat"] = listed
            more = len(stat) - len(listed)
        else:
            more = 0
        budget = limits["bytes"] - 768 - len(json.dumps(result).encode())
        pieces, used, taken, notes = [], 0, 0, []
        stream = _hunks(checkout, commit, spec, untracked)
        for number, line in enumerate(stream):
            if number < offset:
                continue
            clip = line.encode("utf-8")
            if len(clip) > limits["line_bytes"]:
                line = clip[:limits["line_bytes"]].decode("utf-8", errors="ignore").rstrip("\r\n") + \
                    f" [line cut at {limits['line_bytes']} bytes]\n"
            size = len(json.dumps(line, ensure_ascii=True).encode())
            if taken == limits["read_lines"] or used + size > budget:
                stream.close()
                break
            pieces.append(line)
            used += size
            taken += 1
        else:
            stream = None
        result["diff"] = "".join(pieces)
        if stream is not None:
            result["next_offset"] = offset + taken
            notes.append(f"Truncated. Continue with offset={offset + taken}.")
        else:
            result["next_offset"] = None
        if more:
            notes.append(f"{more} more changed files are not listed; narrow the path to see them.")
        if extra:
            notes.append(f"{len(extra)} more untracked files are not listed; narrow the path.")
        result["message"] = " ".join(notes)
        return result

    return diff
