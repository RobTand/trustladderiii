"""The read-only diff tool of the maker and reviewer (#434)."""

import copy
from importlib.resources import files
import json
import os
from pathlib import Path
import signal
import subprocess

from langchain_core.language_models.fake_chat_models import FakeMessagesListChatModel
from langchain_core.messages import AIMessage
import pytest

from tl3_workers import diff as diff_module
from tl3_workers import forecast, runtime
from tl3_workers.roles import ROLES
from tl3_workers.tools import OUTPUT_LIMITS, READ_TOOLS, TOOL_FACTORIES, Worktree, build_tools
from message_invariants import maker_request, turn  # noqa: F401
from test_reviewer import APPROVE, review_request  # noqa: F401


def git(root, *args):
    env = {"PATH": os.defpath, "GIT_CONFIG_GLOBAL": os.devnull, "GIT_CONFIG_SYSTEM": os.devnull,
           "GIT_AUTHOR_NAME": "T", "GIT_AUTHOR_EMAIL": "t@example.invalid",
           "GIT_COMMITTER_NAME": "T", "GIT_COMMITTER_EMAIL": "t@example.invalid"}
    return subprocess.run(["git", *args], cwd=root, env=env, check=True, text=True,
                          stdout=subprocess.PIPE, stderr=subprocess.DEVNULL).stdout.strip()


APP = "".join(f"line {number}\n" for number in range(1, 11))


@pytest.fixture
def repo(tmp_path):
    """A base commit, one committed change on top of it, and uncommitted edits on top of that."""
    root = tmp_path / "repo"
    (root / "src").mkdir(parents=True)
    git(root, "init", "--quiet", "--template=")
    (root / "src/app.py").write_text(APP)
    (root / "src/old.py").write_text("old\n")
    (root / "docs").mkdir()
    (root / "docs/guide.md").write_text("guide\n")
    (root / "data.bin").write_bytes(b"\x00\x01\x02base")
    git(root, "add", ".")
    git(root, "commit", "--quiet", "-m", "Base")
    base = git(root, "rev-parse", "HEAD")
    (root / "docs/guide.md").write_text("guide\nrewritten in round 0\n")
    git(root, "commit", "--quiet", "-am", "Round 0")
    (root / "src/app.py").write_text(APP.replace("line 3\n", "line three\n") + "line 11\n")
    (root / "src/old.py").unlink()
    (root / "src/new.py").write_text("def added():\n    return 1\n")
    (root / "new.bin").write_bytes(b"\x00\x01\x02new")
    return root, base


def attach(worktree, root, base):
    worktree.diff_checkout = Path(root)
    worktree.diff_base = base
    return worktree


def diff_tool(worktree):
    assert "diff" in TOOL_FACTORIES, "the diff tool is not registered"
    return {tool.name: tool for tool in build_tools(worktree, {"diff": "Diff."})}["diff"]


def failed(tool, **args):
    return tool.invoke({"name": "diff", "args": args, "id": "fixture", "type": "tool_call"})


def test_diff_is_registered_for_the_maker_and_the_reviewer_only():
    assert "diff" in TOOL_FACTORIES and "diff" in READ_TOOLS
    assert {name for name, role in ROLES.items() if "diff" in role.tools} == {"maker", "reviewer"}


def test_diff_shows_a_stat_and_hunks_against_the_base(repo):
    root, base = repo
    with attach(Worktree(root), root, base) as worktree:
        result = diff_tool(worktree).invoke({})
    assert result["base"] == base[:12]
    assert {(item["path"], item["status"], item["added"], item["deleted"]) for item in result["stat"]} == {
        ("docs/guide.md", "M", 1, 0), ("src/app.py", "M", 2, 1), ("src/old.py", "D", 0, 1),
        ("src/new.py", "A", 2, 0), ("new.bin", "A", None, None),
    }
    text = result["diff"]
    assert "diff --git a/src/app.py b/src/app.py" in text
    assert "-line 3\n+line three\n" in text and "+line 11\n" in text
    assert "diff --git a/src/old.py b/src/old.py" in text and "deleted file mode" in text
    assert "diff --git a/src/new.py b/src/new.py" in text and "new file mode" in text and "+def added():\n" in text
    assert "+rewritten in round 0\n" in text
    assert "Binary files" in text
    assert result["next_offset"] is None and result["message"] == ""


def test_diff_covers_committed_and_uncommitted_changes_since_the_base(repo):
    root, base = repo
    with attach(Worktree(root), root, base) as worktree:
        committed = diff_tool(worktree).invoke({"path": "docs"})
    assert [item["path"] for item in committed["stat"]] == ["docs/guide.md"]


@pytest.mark.parametrize("path,paths", [
    ("src/app.py", ["src/app.py"]),
    ("src", ["src/app.py", "src/new.py", "src/old.py"]),
    ("src/new.py", ["src/new.py"]),
    ("data.bin", []),
])
def test_diff_is_scoped_by_path(repo, path, paths):
    root, base = repo
    with attach(Worktree(root), root, base) as worktree:
        result = diff_tool(worktree).invoke({"path": path})
    assert sorted(item["path"] for item in result["stat"]) == paths
    assert ("diff --git" in result["diff"]) is bool(paths)
    if not paths:
        assert result["diff"] == "" and result["message"] == f"No changes under {path}."


def test_diff_pages_a_long_change_like_read(repo):
    root, base = repo
    (root / "big.txt").write_text("".join(f"row {number}\n" for number in range(1, 701)))
    with attach(Worktree(root), root, base) as worktree:
        tool = diff_tool(worktree)
        pages, offset = [], 0
        while offset is not None:
            page = tool.invoke({"path": "big.txt", "offset": offset})
            assert len(json.dumps(page).encode()) <= OUTPUT_LIMITS["bytes"]
            assert len(page["diff"].splitlines()) <= OUTPUT_LIMITS["read_lines"]
            assert bool(page.get("stat")) is (offset == 0)
            pages.append(page)
            offset = page["next_offset"]
            if offset is not None:
                assert page["message"] == f"Truncated. Continue with offset={offset}."
        whole = "".join(page["diff"] for page in pages)
    assert len(pages) > 3
    assert whole.count("\n+row ") == 700 and "+row 700\n" in whole and "+row 1\n" in whole


def test_diff_clips_a_long_line_with_a_notice(repo):
    root, base = repo
    (root / "wide.txt").write_text("x" * 5000 + "\n")
    with attach(Worktree(root), root, base) as worktree:
        text = diff_tool(worktree).invoke({"path": "wide.txt"})["diff"]
    longest = max(len(line.encode()) for line in text.splitlines())
    assert longest <= OUTPUT_LIMITS["line_bytes"] + 64
    assert "[line cut at 2048 bytes]" in text


@pytest.mark.parametrize("path,message", [
    ("/etc/passwd", "Use a relative worktree path"), ("../outside", "Parent traversal is forbidden"),
    ("src/../../x", "Parent traversal is forbidden"), (".git/config", "Git metadata paths are forbidden"),
    ("src\\app.py", "Use a relative worktree path"), ("bad\x00path", "Use a relative worktree path"),
])
def test_diff_refuses_paths_outside_the_worktree(repo, path, message):
    root, base = repo
    with attach(Worktree(root), root, base) as worktree:
        result = failed(diff_tool(worktree), path=path)
    assert result.status == "error" and result.text == message


def test_diff_with_a_negative_offset_is_refused(repo):
    root, base = repo
    with attach(Worktree(root), root, base) as worktree:
        assert failed(diff_tool(worktree), offset=-1).text == "Offset must be nonnegative"


def test_diff_does_not_follow_a_symlink_out_of_the_worktree(repo, tmp_path):
    root, base = repo
    (tmp_path / "secret.txt").write_text("external fixture\n")
    (root / "link").symlink_to(tmp_path / "secret.txt")
    with attach(Worktree(root), root, base) as worktree:
        result = diff_tool(worktree).invoke({})
    assert "external fixture" not in json.dumps(result)
    assert any(item["path"] == "link" for item in result["stat"])


def test_diff_names_a_base_that_is_not_in_the_worktree(repo):
    root, _ = repo
    with attach(Worktree(root), root, "f" * 40) as worktree:
        result = failed(diff_tool(worktree))
    assert result.status == "error" and result.text == "The base commit is not in this worktree."


def test_diff_says_when_no_checkout_is_attached(repo):
    root, _ = repo
    with Worktree(root) as worktree:
        result = failed(diff_tool(worktree))
    assert result.status == "error" and result.text == "diff is not available in this run."


def snapshot(root):
    state = git(root, "-c", "core.fsmonitor=false", "status", "--porcelain=v1", "-uall")
    index = (root / ".git/index").read_bytes()
    objects = sorted(str(path.relative_to(root)) for path in (root / ".git/objects").rglob("*") if path.is_file())
    return state, index, objects, sorted(str(path.relative_to(root)) for path in root.rglob("*")
                                         if ".git" not in path.parts)


def test_diff_is_read_only_and_ignores_repository_diff_configuration(repo):
    root, base = repo
    marker = root.parent / "executed"
    script = root.parent / "evil.sh"
    script.write_text(f"#!/bin/sh\ntouch {marker}\n")
    script.chmod(0o755)
    git(root, "config", "diff.external", str(script))
    git(root, "config", "diff.evil.textconv", str(script))
    git(root, "config", "core.fsmonitor", str(script))
    (root / ".gitattributes").write_text("*.py diff=evil\n")
    before = snapshot(root)
    with attach(Worktree(root), root, base) as worktree:
        tool = diff_tool(worktree)
        assert "diff --git a/src/app.py" in tool.invoke({})["diff"]
        tool.invoke({"path": "src"})
    assert not marker.exists()
    assert snapshot(root) == before


def test_diff_gives_a_clean_worktree_an_empty_change(repo):
    root, _ = repo
    git(root, "add", "-A")
    git(root, "commit", "--quiet", "-m", "Round 1")
    with attach(Worktree(root), root, git(root, "rev-parse", "HEAD")) as worktree:
        result = diff_tool(worktree).invoke({})
    assert result["stat"] == [] and result["diff"] == "" and result["message"] == "No changes against the base."


# Repository configuration and attributes must not run a program, and the tool must be bounded
# in processes, memory and files, whatever the worktree holds (#442 review).
def marker_script(root):
    marker = root.parent / "executed"
    script = root.parent / "evil.sh"
    script.write_text(f"#!/bin/sh\ntouch {marker}\ncat\n")
    script.chmod(0o755)
    return marker, script


def hostile(root, script, driver, attributes, via):
    """Define filter.evil.<driver> and select it for every path, after the fixture's commits."""
    key = f"filter.evil.{driver}"
    if via == "config":
        git(root, "config", key, str(script))
    else:
        extra = root.parent / "extra.gitconfig"
        extra.write_text(f'[filter "evil"]\n\t{driver} = {script}\n')
        git(root, "config", "include.path", str(extra))
    if attributes == "worktree":
        (root / ".gitattributes").write_text("* filter=evil\n")
    else:
        (root / ".git/info").mkdir(exist_ok=True)
        (root / ".git/info/attributes").write_text("* filter=evil\n")


@pytest.mark.parametrize("driver,attributes,via", [
    ("clean", "worktree", "config"), ("process", "worktree", "config"),
    ("clean", "info", "config"), ("clean", "worktree", "include"), ("process", "info", "include"),
])
def test_diff_does_not_run_filter_drivers(repo, driver, attributes, via):
    root, base = repo
    marker, script = marker_script(root)
    hostile(root, script, driver, attributes, via)
    assert not marker.exists()
    with attach(Worktree(root), root, base) as worktree:
        tool = diff_tool(worktree)
        everything = tool.invoke({})
        tracked = tool.invoke({"path": "src/app.py"})
        untracked = tool.invoke({"path": "src/new.py"})
    assert not marker.exists(), "a filter driver ran"
    assert "+line three\n" in tracked["diff"] and "+def added():\n" in untracked["diff"]
    assert {"src/app.py", "src/new.py"} <= {item["path"] for item in everything["stat"]}


def test_diff_refuses_a_filter_name_it_cannot_clear(repo):
    root, base = repo
    marker, script = marker_script(root)
    git(root, "config", "filter.a=b.clean", str(script))
    (root / ".gitattributes").write_text("* filter=a=b\n")
    with attach(Worktree(root), root, base) as worktree:
        result = failed(diff_tool(worktree))
    assert not marker.exists()
    assert result.status == "error"
    assert result.text == "diff is unavailable: the repository defines a filter driver whose name Git cannot be told to disable."


class Tracked:
    """A pipe wrapper that records every read, so a test can see what the tool buffers."""

    def __init__(self, pipe, sizes):
        self.pipe, self.sizes = pipe, sizes

    def readline(self, limit=-1):
        data = self.pipe.readline(limit)
        self.sizes.append(len(data))
        return data

    def __iter__(self):
        for line in self.pipe:
            self.sizes.append(len(line))
            yield line

    def __getattr__(self, name):
        return getattr(self.pipe, name)


def track_reads(monkeypatch):
    sizes, starts = [], []
    real = subprocess.Popen

    class Popen(real):
        def __init__(self, *args, **kwargs):
            super().__init__(*args, **kwargs)
            starts.append(args[0])
            if self.stdout is not None:
                self.stdout = Tracked(self.stdout, sizes)

    monkeypatch.setattr(diff_module.subprocess, "Popen", Popen)
    return sizes, starts


def test_diff_buffers_only_a_bounded_slice_of_a_very_long_line(repo, monkeypatch):
    root, base = repo
    (root / "minified.js").write_text("x" * (8 * 1_048_576) + "\n")
    sizes, _ = track_reads(monkeypatch)
    with attach(Worktree(root), root, base) as worktree:
        text = diff_tool(worktree).invoke({"path": "minified.js"})["diff"]
    assert "[line cut at 2048 bytes]" in text
    assert max(sizes) <= 65_536, f"read {max(sizes)} bytes of one line at once"


def git_processes(monkeypatch):
    count = [0]
    real = subprocess.Popen

    class Popen(real):  # subprocess.run starts its child through Popen too
        def __init__(self, *args, **kwargs):
            count[0] += 1
            super().__init__(*args, **kwargs)

    monkeypatch.setattr(diff_module.subprocess, "Popen", Popen)
    return count


def test_a_page_costs_the_same_number_of_git_processes_however_many_files_changed(repo, monkeypatch):
    root, base = repo
    counts = {}
    for name, files_count in (("few", 3), ("many", 150)):
        for number in range(files_count):
            (root / f"{name}_{number}.txt").write_text(f"{name} {number}\n")
        count = git_processes(monkeypatch)
        with attach(Worktree(root), root, base) as worktree:
            tool = diff_tool(worktree)
            count[0] = 0
            tool.invoke({"offset": 40})
        counts[name] = count[0]
        monkeypatch.undo()
        for number in range(files_count):
            (root / f"{name}_{number}.txt").unlink()
    assert counts["many"] == counts["few"] and counts["few"] <= 10


def test_diff_lists_and_pages_at_most_200_untracked_files(repo):
    root, base = repo
    for number in range(230):
        (root / f"u{number:03}.txt").write_text(f"u {number}\n")
    with attach(Worktree(root), root, base) as worktree:
        tool = diff_tool(worktree)
        first = tool.invoke({})
        text, offset = first["diff"], first["next_offset"]
        while offset is not None:
            page = tool.invoke({"offset": offset})
            text, offset = text + page["diff"], page["next_offset"]
    # The fixture has two untracked files that sort first, so 198 of the new ones fit in the 200.
    assert text.count("\ndiff --git a/u") == 198
    assert "32 more untracked files are not shown; narrow the path." in first["message"]


def test_diff_skips_files_that_are_not_regular_or_are_too_large(repo):
    root, base = repo
    os.mkfifo(root / "pipe")
    (root / "docs/guide.md").unlink()
    os.mkfifo(root / "docs/guide.md")
    (root / "huge.txt").write_bytes(b"a\n" * (diff_module.DIFF_FILE_BYTES // 2 + 1))
    previous = signal.signal(signal.SIGALRM, lambda *_: (_ for _ in ()).throw(TimeoutError("diff hung")))
    signal.alarm(60)
    try:
        with attach(Worktree(root), root, base) as worktree:
            result = diff_tool(worktree).invoke({})
    finally:
        signal.alarm(0)
        signal.signal(signal.SIGALRM, previous)
    assert {"pipe", "huge.txt", "docs/guide.md"}.isdisjoint(item["path"] for item in result["stat"])
    assert "src/app.py" in {item["path"] for item in result["stat"]}
    # Git's own file listing omits an untracked FIFO, so only the tracked one and the huge file are named.
    assert "2 changed files were skipped because they are not regular files or are over 16,777,216 bytes" in result["message"]
    for name in ("docs/guide.md", "huge.txt"):
        assert name in result["message"]


def test_the_size_cap_is_the_planners_declared_file_cap():
    assert diff_module.DIFF_FILE_BYTES == forecast.HARD_CAP_BYTES


# The maker and reviewer runs attach the checkout and the base the tool reads.
def tool_results(monkeypatch, request, rounds, reply):
    """Run a role through one tool-call turn per entry in rounds, then reply; return the tool messages."""
    observed = []

    class Model(FakeMessagesListChatModel):
        def _generate(self, messages, **kwargs):
            observed.append(copy.deepcopy(messages))
            return super()._generate(messages, **kwargs)

    monkeypatch.setattr(runtime, "create_model", lambda _: Model(responses=[
        *(AIMessage(**turn(calls=[call])) for call in rounds), AIMessage(**turn(reply)),
    ]))
    events = []
    runtime.run(request, events.append)
    return [message for message in observed[-1] if message.type == "tool"]


def test_the_maker_diffs_its_worktree_against_check_base_sha(maker_request, monkeypatch):  # noqa: F811
    root = Path(maker_request["worktree"])
    maker_request["inputs"]["check_base_sha"] = git(root, "rev-parse", "HEAD")
    write = {"name": "write", "args": {"path": "greeting.py", "content": "def greet(): return 'hello'\n"},
             "id": "write", "type": "tool_call"}
    diff = {"name": "diff", "args": {}, "id": "diff", "type": "tool_call"}
    written, shown = tool_results(monkeypatch, maker_request, [write, diff], {"kind": "cant_meet", "report": "x"})
    assert written.status == "success" and shown.status == "success"
    body = json.loads(shown.content)
    assert body["base"] == maker_request["inputs"]["check_base_sha"][:12]
    assert "-def greet(): return 'goodbye'\n+def greet(): return 'hello'\n" in body["diff"]


def test_the_reviewer_diffs_the_worktree_against_base_sha(review_request, monkeypatch):  # noqa: F811
    from tl3_workers.roles import reviewer
    monkeypatch.setattr(reviewer, "validate_reply", lambda *args: None)
    root = Path(review_request["worktree"])
    git(root, "init", "--quiet", "--template=")
    git(root, "add", ".")
    git(root, "commit", "--quiet", "-m", "Base")
    base = git(root, "rev-parse", "HEAD")
    (root / "greeting.py").write_text('def greet():\n    return "hello"\n')
    git(root, "commit", "--quiet", "-am", "Head")
    review_request["inputs"].update(base_sha=base, head_sha=git(root, "rev-parse", "HEAD"))
    diff = {"name": "diff", "args": {"path": "greeting.py"}, "id": "diff", "type": "tool_call"}
    approve = {**APPROVE, "head_sha": review_request["inputs"]["head_sha"]}
    shown, = tool_results(monkeypatch, review_request, [diff], approve)
    assert shown.status == "success"
    body = json.loads(shown.content)
    assert body["base"] == base[:12]
    assert '-    return "goodbye"\n+    return "hello"\n' in body["diff"]


def prompt_text(name):
    return " ".join(files("tl3_workers").joinpath("prompts", name).read_text().split())


def test_prompts_and_descriptions_state_what_the_diff_tool_shows():
    maker = prompt_text("maker.md")
    reviewer = prompt_text("reviewer.md")
    # #648: the prompts leave diff's arguments to its tool description.
    assert "read the final diff" in maker
    assert "Read the surrounding code when the diff alone does not prove a claim" in reviewer
    assert "check_base_sha, as a stat and unified hunks, paged" in ROLES["maker"].tools["diff"]
    assert "from base_sha to head_sha, as a stat and unified hunks, paged" in ROLES["reviewer"].tools["diff"]
    for role, said in (("maker", "against check_base_sha"), ("reviewer", "from base_sha to head_sha")):
        description = ROLES[role].tools["diff"]
        assert said in description and "Read-only" in description
