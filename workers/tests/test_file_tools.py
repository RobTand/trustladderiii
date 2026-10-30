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


@pytest.mark.parametrize("name,args,expected", [
    ("edit", {"path": "src/code.py", "old": "hello", "new": "hi"},
     "Expected a unique match, or replace_all"),
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


def test_os_error_does_not_expose_host_path(tree, monkeypatch):
    root, _ = tree
    with Worktree(root) as worktree:
        tools = tools_for(worktree)
        def denied(*args, **kwargs):
            raise PermissionError("host private path /controller/fixture")
        monkeypatch.setattr(os, "open", denied)
        result = invoke(tools["read"], "src/code.py")
    assert result.status == "error"
    assert result.text == "Worktree tool rejected the path or operation."


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
