import json
import os

import pytest

from tl3_workers.tools import READ_TOOLS, TOOL_NAMES, WRITE_TOOLS, Worktree, build_tools


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


def tools_for(worktree, names=TOOL_NAMES):
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
        assert tools["list"].invoke({}) == [{"path": "src", "is_dir": True}]
        assert tools["read"].invoke({"path": "src/code.py", "offset": 1, "limit": 1}) == "hello again\n"
        assert tools["glob"].invoke({"pattern": "**/*.py"}) == ["src/code.py"]
        assert tools["glob"].invoke({"pattern": "*.py", "path": "src"}) == ["src/code.py"]
        assert tools["grep"].invoke({"pattern": "again"}) == [
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


@pytest.mark.parametrize("name", sorted(TOOL_NAMES))
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


@pytest.mark.parametrize("name", sorted(TOOL_NAMES))
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
        assert "link" not in tools["glob"].invoke({"pattern": "**/*"})
        assert tools["grep"].invoke({"pattern": "external fixture"}) == []
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


@pytest.mark.parametrize("name", sorted(TOOL_NAMES))
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
