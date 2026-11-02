from concurrent.futures import ThreadPoolExecutor
from contextlib import contextmanager
import errno
import json
import os

import pytest

from tl3_workers.tools import OUTPUT_LIMITS, READ_TOOLS, TOOL_FACTORIES, WRITE_TOOLS, Worktree, build_tools


@pytest.fixture
def tree(tmp_path):
    root = tmp_path / "worktree"
    root.mkdir()
    outside = tmp_path / "outside"
    outside.mkdir()
    (outside / "secret.txt").write_text("external fixture\n")
    (root / "src").mkdir()
    (root / "src/code.py").write_text("hello\nhello again\n")
    return root, outside


def tools_for(worktree, names=READ_TOOLS | WRITE_TOOLS):
    return {tool.name: tool for tool in build_tools(
        worktree, {name: f"Fixture description for {name}." for name in names},
    )}


def invoke(tool, path):
    args = {"path": path}
    if tool.name == "glob":
        args["pattern"] = "**/*"
    elif tool.name == "grep":
        args["pattern"] = "external fixture"
    elif tool.name == "write":
        args["content"] = "changed"
    elif tool.name == "edit":
        args.update(old="external fixture", new="changed")
    return tool.invoke({"name": tool.name, "args": args, "id": "fixture", "type": "tool_call"})


def test_all_file_operations(tree):
    root, _ = tree
    with Worktree(root) as worktree:
        tools = tools_for(worktree)
        assert set(tools) == READ_TOOLS | WRITE_TOOLS
        assert tools["list"].invoke({})["entries"] == [{"path": "src", "is_dir": True}]
        assert tools["read"].invoke({"path": "src/code.py", "offset": 1, "limit": 1}) == "hello again\n"
        assert tools["glob"].invoke({"pattern": "**/*.py"})["entries"] == ["src/code.py"]
        assert tools["glob"].invoke({"pattern": "*.py", "path": "src"})["entries"] == ["src/code.py"]
        assert tools["grep"].invoke({"pattern": "again"})["matches"] == [
            {"path": "src/code.py", "line": 2, "text": "hello again"},
        ]
        tools["write"].invoke({"path": "new/deep/file.txt", "content": "long original\n"})
        assert (root / "new/deep/file.txt").read_text() == "long original\n"
        tools["write"].invoke({"path": "new/deep/file.txt", "content": "short"})
        assert (root / "new/deep/file.txt").read_text() == "short"
        tools["edit"].invoke({"path": "new/deep/file.txt", "old": "short", "new": "x"})
        assert (root / "new/deep/file.txt").read_text() == "x"
        failed = invoke(tools["edit"], "src/code.py")
        assert failed.status == "error"
        tools["edit"].invoke({"path": "src/code.py", "old": "hello", "new": "hi", "replace_all": True})
        assert (root / "src/code.py").read_text() == "hi\nhi again\n"


@pytest.mark.parametrize("name", sorted(READ_TOOLS | WRITE_TOOLS))
@pytest.mark.parametrize("escape", [
    "absolute-outside", "absolute-inside", "../outside/secret.txt",
    "src/../../outside/secret.txt", "src/../code.py", "//outside/secret.txt",
    "src/../../../outside/new/file.txt", "bad\x00path", "C:\\outside\\secret.txt",
])
def test_each_tool_rejects_absolute_and_parent_paths(tree, name, escape):
    root, outside = tree
    if escape == "absolute-outside":
        escape = str(outside / "secret.txt")
    elif escape == "absolute-inside":
        escape = str(root / "src/code.py")
    with Worktree(root) as worktree:
        result = invoke(tools_for(worktree)[name], escape)
    assert result.status == "error"
    assert "external fixture" not in result.text
    assert (outside / "secret.txt").read_text() == "external fixture\n"
    assert not (outside / "new").exists()


@pytest.mark.parametrize("name", sorted(READ_TOOLS | WRITE_TOOLS))
@pytest.mark.parametrize("when", ["before", "during"])
@pytest.mark.parametrize("kind", ["file", "directory", "dangling", "chain", "loop", "internal"])
def test_symlinks_are_rejected_even_when_created_during_run(tree, name, when, kind):
    root, outside = tree

    def link():
        targets = {
            "file": outside / "secret.txt", "directory": outside,
            "dangling": outside / "new.txt", "chain": root / "second",
            "loop": root / "link", "internal": root / "src",
        }
        if kind == "chain":
            (root / "second").symlink_to(outside)
        (root / "link").symlink_to(targets[kind])

    if when == "before":
        link()
    with Worktree(root) as worktree:
        tools = tools_for(worktree)
        if when == "during":
            link()
        path = "link/secret.txt" if kind in {"directory", "chain", "internal"} and name in WRITE_TOOLS | {"read"} else "link"
        result = invoke(tools[name], path)
        assert result.status == "error"
        assert "external fixture" not in result.text
        assert "external fixture" not in json.dumps(tools["list"].invoke({}))
        assert "link" not in tools["glob"].invoke({"pattern": "**/*"})["entries"]
        assert tools["grep"].invoke({"pattern": "external fixture"})["matches"] == []
    assert (outside / "secret.txt").read_text() == "external fixture\n"
    assert not (outside / "new.txt").exists()


@pytest.mark.parametrize("pattern", ["../**", "/**", "src/../../**", "**/../*", "bad\x00*"])
def test_glob_rejects_escape_in_pattern(tree, pattern):
    root, _ = tree
    with Worktree(root) as worktree:
        result = tools_for(worktree)["glob"].invoke({
            "name": "glob", "id": "fixture", "type": "tool_call",
            "args": {"pattern": pattern},
        })
        assert result.status == "error"


@pytest.mark.parametrize("name", sorted(READ_TOOLS | WRITE_TOOLS))
def test_component_replaced_by_symlink_at_open_is_rejected(tree, name, monkeypatch):
    root, outside = tree
    real_open = os.open
    with Worktree(root) as worktree:
        tools = tools_for(worktree)

        def swap(path, flags, *args, **kwargs):
            if path == "src" and kwargs.get("dir_fd") is not None:
                (root / "src").rename(root / "old-src")
                (root / "src").symlink_to(outside)
            return real_open(path, flags, *args, **kwargs)

        monkeypatch.setattr(os, "open", swap)
        path = "src/secret.txt" if name in WRITE_TOOLS | {"read"} else "src"
        result = invoke(tools[name], path)
        assert result.status == "error"
    assert (outside / "secret.txt").read_text() == "external fixture\n"


@pytest.mark.parametrize("name", ["read", "write", "edit"])
def test_final_component_replaced_by_symlink_at_open_is_rejected(tree, name, monkeypatch):
    root, outside = tree
    real_open = os.open
    with Worktree(root) as worktree:
        tools = tools_for(worktree)

        def swap(path, flags, *args, **kwargs):
            if path == "code.py" and kwargs.get("dir_fd") is not None:
                (root / "src/code.py").unlink()
                (root / "src/code.py").symlink_to(outside / "secret.txt")
            return real_open(path, flags, *args, **kwargs)

        monkeypatch.setattr(os, "open", swap)
        assert invoke(tools[name], "src/code.py").status == "error"
    assert (outside / "secret.txt").read_text() == "external fixture\n"


@pytest.mark.parametrize("name", ["read", "write", "edit"])
@pytest.mark.parametrize("kind", ["hardlink", "fifo"])
def test_non_regular_and_hardlinked_files_are_rejected(tree, name, kind):
    root, outside = tree
    if kind == "hardlink":
        os.link(outside / "secret.txt", root / "target")
    else:
        os.mkfifo(root / "target")
    with Worktree(root) as worktree:
        assert invoke(tools_for(worktree)[name], "target").status == "error"
    assert (outside / "secret.txt").read_text() == "external fixture\n"


@pytest.mark.parametrize("name", ["execute", "task", "fetch", "read_file", "delete"])
def test_only_tl3_file_tools_can_be_declared(tree, name):
    root, _ = tree
    with Worktree(root) as worktree:
        with pytest.raises(ValueError, match="Unknown TL3 tools"):
            build_tools(worktree, {name: "forbidden"})


@pytest.mark.parametrize("name", sorted(READ_TOOLS | WRITE_TOOLS))
@pytest.mark.parametrize("path", [".git", ".git/config", "src/.git", "src/.git/hooks/run"])
def test_git_metadata_paths_are_rejected(tree, name, path):
    root, _ = tree
    for directory in (root, root / "src"):
        (directory / ".git").mkdir()
        (directory / ".git/config").write_text("git metadata fixture")
    with Worktree(root) as worktree:
        result = invoke(tools_for(worktree)[name], path)
    assert result.status == "error"
    assert result.text == "Git metadata paths are forbidden"
    assert (root / ".git/config").read_text() == "git metadata fixture"
    assert not (root / "src/.git/hooks").exists()


@pytest.mark.parametrize("pattern", [".git", "**/.git/*", "src/.git/**"])
def test_git_glob_patterns_are_rejected(tree, pattern):
    root, _ = tree
    with Worktree(root) as worktree:
        result = tools_for(worktree)["glob"].invoke({
            "name": "glob", "id": "fixture", "type": "tool_call",
            "args": {"pattern": pattern},
        })
    assert result.status == "error"
    assert result.text == "Git metadata paths are forbidden"


def test_git_metadata_is_omitted_from_enumeration(tree):
    root, _ = tree
    (root / ".git").write_text("gitdir: /external/fixture")
    (root / "src/.git").mkdir()
    (root / "src/.git/config").write_text("git metadata fixture")
    with Worktree(root) as worktree:
        tools = tools_for(worktree)
        assert tools["list"].invoke({})["entries"] == [{"path": "src", "is_dir": True}]
        assert tools["list"].invoke({"path": "src"})["entries"] == [
            {"path": "src/code.py", "is_dir": False},
        ]
        assert tools["glob"].invoke({"pattern": "**/*"})["entries"] == ["src", "src/code.py"]
        assert tools["glob"].invoke({"pattern": "**/[.]git/**"})["entries"] == []
        assert tools["grep"].invoke({"pattern": "git"})["matches"] == []


def test_read_line_limit_and_continuation(tree):
    root, _ = tree
    count = OUTPUT_LIMITS["read_lines"]
    (root / "many").write_text("a\n" * (count + 1))
    with Worktree(root) as worktree:
        output = worktree.read("many", limit=10_000)
        assert output.startswith("a\n" * count + "\n[Truncated.")
        assert f"offset={count}, column=0" in output
        assert worktree.read("many", offset=count) == "a\n"
        assert "Truncated" in worktree.read("many", limit=1)


def test_read_line_byte_limit_and_multibyte_continuation(tree):
    root, _ = tree
    content = "界" * 2_000_000
    (root / "huge").write_text(content)
    with Worktree(root) as worktree:
        first = worktree.read("huge")
        preview, notice = first.split("\n[Truncated.")
        assert len(preview.encode("utf-8")) <= OUTPUT_LIMITS["line_bytes"]
        assert f"offset=0, column={len(preview)}" in notice
        second = worktree.read("huge", column=len(preview))
        assert second.startswith(preview)
        assert len(first.encode("utf-8")) <= OUTPUT_LIMITS["bytes"]
        assert worktree.read("huge", column=len(content) - 1) == "界"


def test_read_total_byte_limit(tree):
    root, _ = tree
    line = "a" * (OUTPUT_LIMITS["line_bytes"] - 1) + "\n"
    (root / "large").write_text(line * OUTPUT_LIMITS["read_lines"])
    with Worktree(root) as worktree:
        output = worktree.read("large", limit=10_000)
    assert len(output.encode("utf-8")) <= OUTPUT_LIMITS["bytes"]
    assert "Truncated. Continue with offset=" in output
    assert output.count("\n") < OUTPUT_LIMITS["read_lines"]


@pytest.mark.parametrize("name,cap,label,args", [
    ("list", "list_entries", "entries", {}),
    ("glob", "glob_entries", "entries", {"pattern": "*.txt"}),
    ("grep", "grep_matches", "matches", {"pattern": "match"}),
])
def test_entry_caps_and_pagination(tree, name, cap, label, args):
    root, _ = tree
    target = root / "pages"
    target.mkdir()
    count = OUTPUT_LIMITS[cap]
    for index in range(count + 1):
        (target / f"{index:04}.txt").write_text("match\n")
    with Worktree(root) as worktree:
        tool = tools_for(worktree)[name]
        first = tool.invoke({"path": "pages", **args})
        assert len(first[label]) == count
        assert first["next_offset"] == count
        assert f"Truncated. Continue with offset={count}" in first["message"]
        last = tool.invoke({"path": "pages", "offset": count, **args})
        assert len(last[label]) == 1
        assert last["next_offset"] is None
        assert not last["message"]
        assert first[label][-1] != last[label][0]


@pytest.mark.parametrize("name,label,args", [
    ("list", "entries", {}), ("glob", "entries", {"pattern": "*"}),
    ("grep", "matches", {"pattern": "match"}),
])
def test_collection_byte_caps(tree, name, label, args):
    root, _ = tree
    target = root / "pages"
    target.mkdir()
    for index in range(100):
        (target / (f"{index:04}" + "界" * 60)).write_text("match" + "界" * 1000)
    with Worktree(root) as worktree:
        tool = tools_for(worktree)[name]
        output = tool.invoke({"path": "pages", **args})
        assert 0 < len(output[label]) < 100
        assert output["next_offset"] == len(output[label])
        assert "Truncated" in output["message"]
        assert len(json.dumps(output).encode("utf-8")) <= OUTPUT_LIMITS["bytes"]
        rest = tool.invoke({"path": "pages", "offset": output["next_offset"], **args})
        assert rest[label][0] != output[label][-1]


def test_grep_long_line_preview_points_to_read(tree):
    root, _ = tree
    (root / "large").write_text("match" + "界" * 10000)
    with Worktree(root) as worktree:
        match = worktree.grep("match")["matches"][0]
    preview, notice = match["text"].split(" [Truncated.")
    assert len(preview.encode("utf-8")) <= OUTPUT_LIMITS["line_bytes"]
    assert "Use read offset=0, column=0" in notice


@pytest.mark.parametrize("ending", ["\n", "\r", "\r\n", ""])
def test_grep_reads_bounded_chunks_and_finds_tail_and_boundary_matches(tree, monkeypatch, ending):
    root, _ = tree
    cap = OUTPUT_LIMITS["line_bytes"]
    content = "x" * (cap - 1) + "needle" + "界" * 100_000 + "tail" + ending
    (root / "huge").write_bytes(content.encode("utf-8"))
    if ending:
        with (root / "huge").open("ab") as stream:
            stream.write(("needle" + ending).encode("utf-8"))
    sizes = []
    with Worktree(root) as worktree:
        original = worktree._file

        @contextmanager
        def bounded_file(path):
            with original(path) as stream:
                class Bounded:
                    def __iter__(self):
                        raise AssertionError("grep must use bounded readline")

                    def readline(self, size=-1):
                        assert 0 < size <= cap + 1
                        sizes.append(size)
                        return stream.readline(size)

                    def read(self, size=-1):
                        assert size == 1
                        return stream.read(size)

                    def tell(self):
                        return stream.tell()

                    def seek(self, position):
                        return stream.seek(position)
                yield Bounded()

        monkeypatch.setattr(worktree, "_file", bounded_file)
        for pattern in ("needle", "tail", "界" * (cap * 2), "missing"):
            result = worktree.grep(pattern)
            matches = [match for match in result["matches"] if match["path"] == "huge"]
            assert [match["line"] for match in matches] == (
                [] if pattern == "missing" else [1, 2] if pattern == "needle" and ending else [1]
            )
            if matches:
                assert "Truncated. Use read offset=0, column=0." in matches[0]["text"]
    assert len(sizes) > 100


def test_grep_crlf_split_at_chunk_boundary_preserves_line_numbers(tree):
    root, _ = tree
    cap = OUTPUT_LIMITS["line_bytes"]
    (root / "endings").write_bytes(("x" * cap + "\r\nneedle\r\n").encode("utf-8"))
    with Worktree(root) as worktree:
        assert worktree.grep("needle")["matches"] == [
            {"path": "endings", "line": 2, "text": "needle"},
        ]


@pytest.mark.parametrize("name,args,expected", [
    ("edit", {"path": "src/code.py", "old": "hello", "new": "hi"},
     "Expected a unique match, or replace_all"),
    ("edit", {"path": "src/code.py", "old": "absent", "new": "hi"}, "No match found"),
    ("edit", {"path": "src/code.py", "old": "absent", "new": "hi", "replace_all": True},
     "No match found"),
    ("read", {"path": "missing"}, "File not found"),
    ("read", {"path": "missing/child"}, "File or directory not found."),
])
def test_static_errors_help_the_model_correct_calls(tree, name, args, expected):
    root, _ = tree
    with Worktree(root) as worktree:
        result = tools_for(worktree)[name].invoke({
            "name": name, "args": args, "id": "fixture", "type": "tool_call",
        })
    assert result.status == "error"
    assert result.text == expected
    assert str(root) not in result.text
    assert (root / "src/code.py").read_text() == "hello\nhello again\n"


def test_os_error_does_not_expose_host_path(tree, monkeypatch):
    root, _ = tree
    with Worktree(root) as worktree:
        tools = tools_for(worktree)
        def denied(*args, **kwargs):
            raise PermissionError("host private path /controller/fixture")
        monkeypatch.setattr(os, "open", denied)
        result = invoke(tools["read"], "src/code.py")
    assert result.status == "error"
    # Q50: an environment failure carries an id a blocked reply can cite.
    assert result.text == "Worktree tool rejected the path or operation. Tool error id: tool-error-1."
    assert "/controller" not in result.text


def test_new_tool_needs_only_a_factory_entry(tree, monkeypatch):
    root, _ = tree
    def factory(worktree):
        def admitted_test(target: str) -> str:
            return "fixture " + target
        return admitted_test
    monkeypatch.setitem(TOOL_FACTORIES, "pbtest_fixture", factory)
    with Worktree(root) as worktree:
        tool = build_tools(worktree, {"pbtest_fixture": "Fixture admitted test tool."})[0]
        assert tool.invoke({"target": "suite"}) == "fixture suite"


@pytest.mark.parametrize("ending", ["\n", "\r", "\r\n"])
def test_read_preserves_line_offsets_and_crlf_at_byte_boundary(tree, ending):
    root, _ = tree
    body = "a" * (OUTPUT_LIMITS["line_bytes"] - 1)
    (root / "endings").write_bytes((body + ending + "next" + ending).encode("utf-8"))
    with Worktree(root) as worktree:
        assert worktree.read("endings", offset=1) == "next" + ending
        first = worktree.read("endings", limit=1)
        if ending == "\r\n":
            preview, notice = first.split("\n[Truncated.")
            assert preview == body
            assert f"offset=0, column={len(body)}" in notice
            assert worktree.read("endings", column=len(body), limit=1) == (
                ending + "\n[Truncated. Continue with offset=1, column=0.]"
            )
        else:
            assert first == body + ending + "\n[Truncated. Continue with offset=1, column=0.]"


def test_concurrent_enumerations_each_see_every_entry(tree):
    # Models make parallel tool calls, which Deep Agents runs in threads.
    root, _ = tree
    names = [f"file{index:02}.py" for index in range(40)]
    for name in names:
        (root / name).write_text("needle\n")
    with Worktree(root) as worktree:
        tools = tools_for(worktree, READ_TOOLS)

        def enumerate_root(_):
            listed = [entry["path"] for entry in tools["list"].invoke({})["entries"]]
            globbed = tools["glob"].invoke({"pattern": "*.py"})["entries"]
            grepped = {match["path"] for match in tools["grep"].invoke({"pattern": "needle"})["matches"]}
            return listed, globbed, grepped

        with ThreadPoolExecutor(max_workers=16) as pool:
            for listed, globbed, grepped in pool.map(enumerate_root, range(400)):
                assert listed == sorted(names + ["src"])
                assert globbed == names
                assert grepped == set(names)


def call(tool, **args):
    """A failing call, as the model sees it: a ToolMessage with status error."""
    return tool.invoke({"name": tool.name, "args": args, "id": "fixture", "type": "tool_call"})


def test_grep_searches_a_single_file_given_as_path(tree):
    # prismaquant#2629: 16 grep calls with a file as path were all rejected.
    root, _ = tree
    (root / "src/other.py").write_text("hello elsewhere\n")
    with Worktree(root) as worktree:
        tool = tools_for(worktree)["grep"]
        assert tool.invoke({"pattern": "hello", "path": "src/code.py"}) == {
            "matches": [{"path": "src/code.py", "line": 1, "text": "hello"},
                        {"path": "src/code.py", "line": 2, "text": "hello again"}],
            "next_offset": None, "message": "",
        }
        assert tool.invoke({"pattern": "absent", "path": "src/code.py"})["matches"] == []
        assert tool.invoke({"pattern": "again", "path": "src/code.py", "offset": 0})["matches"] == [
            {"path": "src/code.py", "line": 2, "text": "hello again"},
        ]


def test_grep_on_a_file_pages_and_truncates_like_a_directory(tree):
    root, _ = tree
    total = OUTPUT_LIMITS["grep_matches"] + 5
    (root / "many.txt").write_text("hit\n" * total)
    cap = OUTPUT_LIMITS["line_bytes"]
    (root / "wide.txt").write_text("needle " + "x" * (cap * 2) + "\n")
    with Worktree(root) as worktree:
        first = worktree.grep("hit", "many.txt")
        assert len(first["matches"]) == OUTPUT_LIMITS["grep_matches"]
        assert first["next_offset"] == OUTPUT_LIMITS["grep_matches"]
        second = worktree.grep("hit", "many.txt", offset=first["next_offset"])
        assert [match["line"] for match in second["matches"]] == list(range(101, total + 1))
        assert second["next_offset"] is None
        wide, = worktree.grep("needle", "wide.txt")["matches"]
        assert wide["path"] == "wide.txt"
        assert wide["text"].endswith("[Truncated. Use read offset=0, column=0.]")


def test_grep_on_a_file_reports_non_utf8_instead_of_an_empty_result(tree):
    root, _ = tree
    (root / "binary.dat").write_bytes(b"\xff\xfe needle\n")
    with Worktree(root) as worktree:
        tool = tools_for(worktree)["grep"]
        assert tool.invoke({"pattern": "needle", "path": "."})["matches"] == []
        result = call(tool, pattern="needle", path="binary.dat")
    assert result.status == "error"
    assert result.text == "Expected valid UTF-8 text."


def test_grep_on_a_missing_path_is_still_not_found(tree):
    root, _ = tree
    with Worktree(root) as worktree:
        result = call(tools_for(worktree)["grep"], pattern="x", path="src/missing.py")
    assert result.status == "error"
    assert result.text == "File or directory not found."


@pytest.mark.parametrize("name,args", [
    ("glob", {"pattern": "*"}), ("list", {}),
])
def test_directory_tools_name_the_file_they_were_given(tree, name, args):
    root, _ = tree
    with Worktree(root) as worktree:
        result = call(tools_for(worktree)[name], path="src/code.py", **args)
    assert result.status == "error"
    assert "src/code.py is a file, not a directory" in result.text
    assert "grep" in result.text and "read" in result.text
    assert "rejected" not in result.text


@pytest.mark.parametrize("name,args", [
    ("read", {"path": "src/code.py/x"}),
    ("write", {"path": "src/code.py/x", "content": "z"}),
    ("grep", {"pattern": "hello", "path": "src/code.py/x"}),
])
def test_a_file_inside_the_path_is_named(tree, name, args):
    root, _ = tree
    with Worktree(root) as worktree:
        result = call(tools_for(worktree)[name], **args)
        assert not worktree.evidence.tool_errors
    assert result.status == "error"
    assert "src/code.py/x" in result.text
    assert "a file, not a directory" in result.text
    assert "rejected" not in result.text


@pytest.mark.parametrize("code,wording", [
    (errno.ENOTDIR, "src/code.py is a file, not a directory"),
    (errno.EISDIR, "is a directory, not a file"),
    (errno.ELOOP, "symbolic link"),
    (errno.ENAMETOOLONG, "too long"),
    (errno.EINVAL, "EINVAL"),
    (None, "could not be used"),
])
def test_model_caused_os_errors_name_the_path_and_the_fix(tree, monkeypatch, code, wording):
    root, _ = tree

    def factory(worktree):
        def read(path: str) -> str:
            """Fail like the model's own input."""
            raise OSError(code, "detail /controller/private") if code else OSError("no errno")
        return read

    monkeypatch.setitem(TOOL_FACTORIES, "read", factory)
    with Worktree(root) as worktree:
        result = call(build_tools(worktree, {"read": "Read."})[0], path="src/code.py")
        assert not worktree.evidence.tool_errors
    assert result.status == "error"
    assert "src/code.py" in result.text
    assert wording in result.text
    assert "rejected" not in result.text
    assert "/controller" not in result.text


@pytest.mark.parametrize("code", [
    errno.EACCES, errno.EPERM, errno.EROFS, errno.ENOSPC, errno.EDQUOT,
    errno.EIO, errno.EMFILE, errno.ENFILE, errno.ENOMEM,
])
def test_environment_errors_keep_the_infrastructure_message(tree, monkeypatch, code):
    root, _ = tree

    def factory(worktree):
        def read(path: str) -> str:
            """Fail like the environment."""
            raise OSError(code, "detail /controller/private")
        return read

    monkeypatch.setitem(TOOL_FACTORIES, "read", factory)
    with Worktree(root) as worktree:
        result = call(build_tools(worktree, {"read": "Read."})[0], path="src/code.py")
        assert worktree.evidence.tool_errors == {
            "tool-error-1": {"tool": "read", "class": errno.errorcode[code], "selectors": None},
        }
    assert result.status == "error"
    assert result.text == "Worktree tool rejected the path or operation. Tool error id: tool-error-1."


@pytest.mark.parametrize("name", ["grep", "list", "glob"])
def test_a_symlink_path_is_named_as_a_link(tree, name):
    root, outside = tree
    (root / "link").symlink_to(outside / "secret.txt")
    args = {"grep": {"pattern": "x"}, "glob": {"pattern": "*"}, "list": {}}[name]
    with Worktree(root) as worktree:
        result = call(tools_for(worktree)[name], path="link", **args)
    assert result.status == "error"
    assert "link is or passes through a symbolic link" in result.text
    assert "external fixture" not in result.text


# grep reads its pattern as a regular expression unless literal is true (#414).
@pytest.fixture
def code_tree(tmp_path):
    root = tmp_path / "worktree"
    (root / "pkg").mkdir(parents=True)
    (root / "pkg/mod.py").write_text(
        "class SupplierRevision(unittest.TestCase):\n"
        "    def test_one(self):\n"
        "        vars(digests)\n"
        "def rung_speed(a):\n"
        "x = foo.bar\n"
        "tab\there\n"
        "last line without newline",
    )
    return root


def lines(result):
    return [match["line"] for match in result["matches"]]


@pytest.mark.parametrize("pattern,expected", [
    ("rung_speed|SupplierRevision|absent_name", [1, 4]),
    ("^class |    def test_", [1, 2]),
    ("here$", [6]),
    ("line without newline$", [7]),
    (r"\bvars\b", [3]),
    (r"def \w+\(\w\):", [4]),
    (r"\d", []),
    (r"foo\.bar", [5]),
    (r"tab\s+here", [6]),
    ("(?i)SUPPLIERREVISION", [1]),
    ("Supplier[A-Z]evision|x = foo", [1, 5]),
])
def test_grep_reads_the_pattern_as_a_regular_expression(code_tree, pattern, expected):
    with Worktree(code_tree) as worktree:
        assert lines(worktree.grep(pattern, "pkg/mod.py")) == expected
        assert lines(worktree.grep(pattern, "pkg")) == expected


def test_grep_regex_is_case_sensitive_and_matches_within_a_line(code_tree):
    with Worktree(code_tree) as worktree:
        assert lines(worktree.grep("supplierrevision")) == []
        assert lines(worktree.grep("Revision")) == [1]


def test_grep_literal_true_searches_the_text_as_written(code_tree):
    with Worktree(code_tree) as worktree:
        assert lines(worktree.grep("vars(digests)", literal=True)) == [3]
        assert lines(worktree.grep("rung_speed|SupplierRevision", literal=True)) == []
        assert lines(worktree.grep("^class ", literal=True)) == []
        assert lines(worktree.grep("foo.bar", literal=True)) == [5]
        assert worktree.grep("rung_speed|x", literal=True)["message"] == ""


def test_grep_literal_defaults_to_false_in_the_tool_schema(code_tree):
    with Worktree(code_tree) as worktree:
        tool = tools_for(worktree)["grep"]
        assert tool.args["literal"]["default"] is False
        assert tool.invoke({"pattern": "rung_speed|SupplierRevision"})["matches"] != []
        assert tool.invoke({"pattern": "rung_speed|SupplierRevision", "literal": True})["matches"] == []


@pytest.mark.parametrize("pattern", ["def test_one(", "vars(digests", "x[0", "*start", "foo)"])
def test_grep_searches_an_invalid_regex_as_literal_text_and_says_so(tmp_path, pattern):
    (tmp_path / "f.py").write_text(f"before\n{pattern}\nafter\n")
    with Worktree(tmp_path) as worktree:
        result = worktree.grep(pattern)
    assert lines(result) == [2]
    assert "not a valid regular expression" in result["message"]
    assert "searched as literal text" in result["message"]
    assert "literal=true" in result["message"]


def test_grep_says_when_a_regex_found_nothing_that_the_text_might_be_literal(code_tree):
    with Worktree(code_tree) as worktree:
        # vars(digests) is a valid regex whose group means something else.
        empty = worktree.grep("vars(digests)")
        assert empty["matches"] == []
        assert "No matches for the regular expression" in empty["message"]
        assert "literal=true" in empty["message"]
        # Plain words and a bare dot cannot differ between the two readings.
        assert worktree.grep("nothing_here")["message"] == ""
        assert worktree.grep("no.thing")["message"] == ""
        assert worktree.grep("rung_speed|nothing_here")["message"] == ""
        # A page past the last match is not "no matches".
        assert worktree.grep("rung_speed|SupplierRevision", offset=2)["matches"] == []
        assert worktree.grep("rung_speed|SupplierRevision", offset=2)["message"] == ""


@pytest.mark.parametrize("pattern,reason", [
    ("(a+)+$", "nested repeat"),
    ("(a*)*b", "nested repeat"),
    ("(a|aa)+$", "alternation inside a repeat"),
    ("(?:ab|a)*c", "alternation inside a repeat"),
    (r"(a)\1", "backreference"),
    ("(?=a)a", "lookaround"),
    ("(?<!a)b", "lookaround"),
    (".*.*x", "adjacent repeats"),
    (".+.*x", "adjacent repeats"),
    (r"\w+.*x", "adjacent repeats"),
    ("a{1,500}", "repeat count"),
    ("a+b+c+d+e+f", "too many repeats"),
    ("x" * 501, "too long"),
])
def test_grep_refuses_patterns_that_can_backtrack_catastrophically(code_tree, pattern, reason):
    with Worktree(code_tree) as worktree:
        result = call(tools_for(worktree)["grep"], pattern=pattern)
        error = pytest.raises(ValueError, worktree.grep, pattern)
    assert result.status == "error"
    assert reason in result.text
    assert "literal=true" in result.text
    assert str(error.value) == result.text


def test_grep_still_searches_a_refused_pattern_as_literal_text(tmp_path):
    (tmp_path / "f.py").write_text("see (a+)+$ here\n")
    with Worktree(tmp_path) as worktree:
        assert lines(worktree.grep("(a+)+$", literal=True)) == [1]


@pytest.mark.parametrize("pattern", [
    r"^\s*def \w+\(.*\):", r"class \w+\(.*Test", r"foo(bar|baz)?qux", r"(?:ab)+c",
    r"\w+\s*=\s*\d+", r"a{1,50}b", r"[a-z_]+\.py$", "x|y|z|w|v|u|t",
])
def test_grep_accepts_ordinary_code_search_patterns(code_tree, pattern):
    with Worktree(code_tree) as worktree:
        worktree.grep(pattern)


def test_grep_regex_looks_only_at_the_first_characters_of_a_long_line(tmp_path):
    cap = OUTPUT_LIMITS["line_bytes"]
    (tmp_path / "wide.txt").write_text("x" * cap + "needle\nneedle at the start\n")
    with Worktree(tmp_path) as worktree:
        assert lines(worktree.grep("need.e")) == [2]
        assert lines(worktree.grep("needle", literal=True)) == [1, 2]
        assert lines(worktree.grep("needle")) == [2]


def test_grep_regex_keeps_paging_and_truncation(tmp_path):
    total = OUTPUT_LIMITS["grep_matches"] + 5
    cap = OUTPUT_LIMITS["line_bytes"]
    (tmp_path / "many.txt").write_text("hit one\nmiss\n" * total)
    (tmp_path / "wide.txt").write_text("needle " + "x" * (cap * 2) + "\n")
    with Worktree(tmp_path) as worktree:
        first = worktree.grep("^hit|never", "many.txt")
        assert len(first["matches"]) == OUTPUT_LIMITS["grep_matches"]
        assert first["next_offset"] == OUTPUT_LIMITS["grep_matches"]
        second = worktree.grep("^hit|never", "many.txt", offset=first["next_offset"])
        assert lines(second) == [2 * n + 1 for n in range(100, total)]
        assert second["next_offset"] is None
        wide, = worktree.grep("^needle", "wide.txt")["matches"]
        assert wide["text"].endswith("[Truncated. Use read offset=0, column=0.]")


def test_grep_regex_handles_crlf_line_endings(tmp_path):
    (tmp_path / "f.txt").write_bytes(b"one\r\ntwo$\r\nthree\r\n")
    with Worktree(tmp_path) as worktree:
        assert lines(worktree.grep("^one$")) == [1]
        assert lines(worktree.grep("^three$")) == [3]
        assert lines(worktree.grep(r"two\$$")) == [2]
