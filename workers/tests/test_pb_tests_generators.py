# ruff: noqa: F811

import hashlib
import json
from pathlib import Path
import subprocess
import pytest
from copy import deepcopy
from pb_fixtures import FLUX_EXECUTION
from pb_fixtures import flux_settings
from pb_fixtures import run_binding
from pb_fixtures import supported_fake_runtime as supported_fake_runtime
from tl3_workers.tools import build_tools

from worker_process import IMAGE
from worker_process import KEY
from worker_process import configured_generator
from worker_process import generator_backend  # noqa: F401
from worker_process import generator_repo  # noqa: F401


@pytest.mark.parametrize('unknown_first', [False, True])
def test_selected_flux_generator_retains_unknown_and_imports_once(generator_repo, tmp_path, monkeypatch, unknown_first):
    from tl3_harness import generator
    from tl3_workers import flux_execution
    tree, generate = configured_generator(generator_repo, tmp_path)
    tree.test_settings = flux_settings()
    root = tmp_path / 'retained-input'
    root.mkdir()
    key = 'worker:' + 'a' * 64
    prepared, attached = [], []
    monkeypatch.setattr(generator, 'require_admission', lambda: None)
    monkeypatch.setattr(generator, 'container_command', lambda tree, container, argv, cidfile: argv)
    monkeypatch.setattr(flux_execution, 'activation', lambda settings: None)
    monkeypatch.setattr(flux_execution, 'allocate', lambda settings: ('a' * 64, root))
    def prepare(settings, call, root, served, contract, **kwargs):
        assert kwargs['generator'] is True
        prepared.append(contract)
        return {'key': key, 'contract': contract}
    monkeypatch.setattr(flux_execution, 'prepare', prepare)
    monkeypatch.setattr(flux_execution, 'resume', lambda settings, reference, served, contract:
                        {'key': reference, 'contract': contract})
    def execute(plan, *, resumed, **kwargs):
        checkpoint = json.loads(tree.generator_state.read_text())
        assert checkpoint['key'] == key and checkpoint['phase'] == 'submitted'
        attached.append(resumed)
        if unknown_first and len(attached) == 1:
            raise ValueError('unknown acceptance; original binding retained')
        artifact = generator.run(root / 'checkout', generator.Container(IMAGE), **plan['contract'])
        return {'kind': 'generator', 'artifact': artifact, 'receipt': {
            'backend': 'flux', 'receipt_sha256': 'b' * 64, 'outer': {'status': 'passed'}}}
    monkeypatch.setattr(flux_execution, 'execute', execute)
    with tree:
        result = generate()
        if unknown_first:
            assert result['status'] == 'failed'
            assert json.loads(tree.generator_state.read_text())['phase'] == 'submitted'
            result = generate()
        assert result['status'] == 'completed', result
        assert generate() == result
    assert len(prepared) == 1
    assert attached == ([False, True] if unknown_first else [False])
    assert root.exists()


@pytest.mark.durability
def test_request_132_git_index_generation_lands_before_independent_check(
        generator_repo, generator_backend, supported_fake_runtime, tmp_path):
    tree, generate = configured_generator(generator_repo, tmp_path)
    with tree:
        # The new files do not occur in the base Git index. The action must add them.
        result = generate()
        assert result["status"] == "completed", result
        assert result["input_tree"] != result["output_tree"]
        assert result["wait_reference"] == KEY
        assert result["artifact_sha256"]
        assert result["receipt_sha256"] == "b" * 64
        assert result["argv"] == ["python3", "generate.py"]
        output = (generator_repo / "tests/BUILD.bazel").read_text()
        assert "tests/socket_support.py:" in output
        assert "tests/test_short_socket.py:tests.socket_support" in output
        assert "tests/test_wait.py:tests.socket_support" in output
        assert len(json.dumps(result)) < 16_384
        subprocess.run(["git", "add", "."], cwd=generator_repo, check=True)
        subprocess.run(["python3", "generate.py", "--check"], cwd=generator_repo, check=True)
        assert generate() == result
        [prepared] = generator_backend[0]
        assert prepared["document"]["walltime_seconds"] == 180
        assert prepared["document"]["image"] == IMAGE
        assert prepared["document"]["generator"] is True
        assert supported_fake_runtime.submitted == [KEY]


@pytest.mark.parametrize('damage', ['none', 'contract', 'source', 'receipt_tree'])
@pytest.mark.durability
def test_exhausted_maker_recovers_completed_generator_receipt_without_resubmission(
        generator_repo, generator_backend, tmp_path, damage):
    from langchain_core.messages import AIMessage, ToolMessage
    from tl3_workers.exploration import ExplorationMiddleware
    from tl3_workers.roles import maker

    original, generate = configured_generator(generator_repo, tmp_path)
    with original:
        receipt = generate()
        assert receipt['status'] == 'completed', receipt
    state_path = original.generator_state
    saved = json.loads(state_path.read_text())
    if damage == 'contract':
        saved['contract']['image'] = 'sha256:' + '9' * 64
    elif damage == 'source':
        (generator_repo / 'tests/socket_support.py').write_text('def short_socket(): return "changed"\n')
    elif damage == 'receipt_tree':
        saved['receipt']['output_tree'] = '0' * 40
    state_path.write_text(json.dumps(saved))

    restored, _ = configured_generator(generator_repo, tmp_path)
    assert not getattr(restored, 'generation_receipt', None)
    # Completed final-tree readback and scope review survive in the visit history.
    calls = [{'name': 'diff', 'args': {}, 'id': 'scope', 'type': 'tool_call'},
             *[{'name': 'read', 'args': {'path': path}, 'id': str(i), 'type': 'tool_call'}
               for i, path in enumerate(restored.generator_files)]]
    history = [AIMessage(content='', tool_calls=calls),
               *[ToolMessage(content='Verified final tree', tool_call_id=call['id']) for call in calls]]
    allowance = ExplorationMiddleware(maker.ROLE.exploration_budget, 'maker')
    allowance.turns = allowance.budget.tool_turns
    allowance.exhaustion = 'backstop'
    request = {'worktree': str(generator_repo), 'operation': {'node': 1, 'generation': 0, 'kind': 'make'},
               'inputs': {'goals': [], 'plan': {'checks': []}}}
    with restored:
        reply = maker._budget_reply(request, allowance, history, restored, {'kind': 'done', 'self_check': []})
    assert len(generator_backend[0]) == 1, 'recovery must never submit generation from the finalization reserve'
    if damage == 'none':
        assert reply['kind'] == 'done', reply
        assert reply['budget_exhaustion']['unfinished_obligations'] == []
        assert restored.generation_receipt == receipt
    else:
        assert reply['kind'] == 'cant_meet', reply
        assert 'Final configured generation' in reply['budget_exhaustion']['unfinished_obligations']


@pytest.mark.parametrize("mutation,fragment", [
    ("Path('tests/test_wait.py').write_text('escaped')", "undeclared"),
    ("Path('tests/BUILD.bazel').unlink()", "deleted"),
    ("Path('tests/BUILD.bazel').write_bytes(b'\\x00')", "text"),
    ("Path('tests/BUILD.bazel').write_bytes(b'\\xff')", "UTF-8"),
    ("Path('tests/BUILD.bazel').write_text('x' * (16 * 1048576 + 1))", "limit"),
    ("raise SystemExit(7)", "7"),
    ("Path('tests/BUILD.bazel').unlink(); Path('tests/BUILD.bazel').symlink_to('/etc/passwd')", "regular"),
    ("Path('tests/BUILD.bazel').chmod(0o755)", "mode"),
])
@pytest.mark.durability
def test_generator_failure_and_out_of_scope_output_preserve_source(
        generator_repo, generator_backend, tmp_path, mutation, fragment):
    script = generator_repo / "generate.py"
    script.write_text(script.read_text() + "\n" + mutation + "\n")
    before = {str(path.relative_to(generator_repo)): path.read_bytes()
              for path in generator_repo.rglob('*') if path.is_file() and '.git' not in path.parts}
    tree, generate = configured_generator(generator_repo, tmp_path)
    with tree:
        result = generate()
    assert result["status"] != "completed", result
    assert fragment in result["message"], result
    assert {name: (generator_repo / name).read_bytes() for name in before} == before


@pytest.mark.parametrize("protected,files,fragment", [
    (["**/BUILD.bazel"], None, "protected"),
])
def test_generator_cannot_override_protection(
        generator_repo, generator_backend, tmp_path, protected, files, fragment):
    tree, generate = configured_generator(generator_repo, tmp_path, protected=protected, files=files)
    with tree:
        result = generate()
    assert result["status"] != "completed"
    assert fragment in result["message"], result


def test_request_2709_test_only_plan_imports_configured_output(generator_repo, generator_backend, tmp_path):
    for name in ("socket_support.py", "test_short_socket.py"):
        (generator_repo / "tests" / name).unlink()
    (generator_repo / "tests/test_wait.py").write_text("from pathlib import Path\n")
    tree, generate = configured_generator(generator_repo, tmp_path, files=["tests/test_wait.py"])
    with tree:
        result = generate()
        assert result["status"] == "completed", result
        assert "tests/test_wait.py:pathlib" in (generator_repo / "tests/BUILD.bazel").read_text()
        assert generate() == result, "verified import remains idempotent"
    assert len(generator_backend[0]) == 1


def test_generator_receipt_is_invalidated_by_source_edit(generator_repo, generator_backend, tmp_path):
    tree, generate = configured_generator(generator_repo, tmp_path)
    with tree:
        assert generate()["status"] == "completed"
        (generator_repo / "tests/socket_support.py").write_text("from pathlib import Path\n")
        assert generate()["status"] == "completed"
    assert len(generator_backend[0]) == 2
    assert "tests/socket_support.py:pathlib" in (generator_repo / "tests/BUILD.bazel").read_text()


def test_generated_outputs_cannot_be_hand_edited(generator_repo, tmp_path):
    tree, _ = configured_generator(generator_repo, tmp_path)
    with tree:
        for method, args in ((tree.write, ("tests/BUILD.bazel", "hand edit")),
                             (tree.edit, ("tests/BUILD.bazel", "socket", "hand edit"))):
            with pytest.raises(ValueError, match="generated"):
                method(*args)


def test_generator_deadline_is_a_failure_not_a_hang(generator_repo, generator_backend, tmp_path):
    from tl3_harness import generator
    import shutil
    staged = tmp_path / "staged"
    shutil.copytree(generator_repo, staged, ignore=shutil.ignore_patterns(".git"))
    result = generator.run(staged, generator.Container(IMAGE),
                           "python3 -c 'import time; time.sleep(60)'", ["**/BUILD.bazel"], [],
                           ["tests/BUILD.bazel"], timeout=0.05)
    assert result["status"] == "failed" and result["exit_code"] == 124
    assert "deadline" in result["message"]


@pytest.mark.parametrize("corruption", ["log", "output", "argv", "incomplete", "stale"])
def test_generator_refuses_unverified_or_stale_artifacts(
        generator_repo, generator_backend, supported_fake_runtime, tmp_path, corruption):
    def corrupt(payload):
        path = Path(payload["artifact"]["path"])
        if corruption == "log":
            path.write_bytes(path.read_bytes() + b"corrupt")
        elif corruption in ("output", "argv"):
            artifact = json.loads(path.read_text())
            if corruption == "output":
                artifact["outputs"][0]["entry"]["sha256"] = "0" * 64
            else:
                artifact["argv"] = ["/bin/sh", "-c", "exit 0"]
            raw = json.dumps(artifact).encode()
            path.write_bytes(raw)
            payload["artifact"].update(bytes=len(raw), sha256=hashlib.sha256(raw).hexdigest())
        else:
            (generator_repo / "tests/socket_support.py").write_text("concurrent source edit\n")

    if corruption == "incomplete":
        supported_fake_runtime.ending = "failed"
    else:
        supported_fake_runtime.on_payload = corrupt
    before = (generator_repo / "tests/BUILD.bazel").read_bytes()
    tree, generate = configured_generator(generator_repo, tmp_path)
    with tree:
        result = generate()
    assert result["status"] != "completed", result
    assert (generator_repo / "tests/BUILD.bazel").read_bytes() == before


@pytest.mark.durability
def test_generator_reattaches_after_interrupted_import(generator_repo, generator_backend, tmp_path, monkeypatch):
    from tl3_workers import generators
    original = generators._import

    def interrupted(tree, outputs):
        original(tree, outputs)
        raise RuntimeError("fixture crash after atomic replacement")

    tree, generate = configured_generator(generator_repo, tmp_path)
    with tree:
        monkeypatch.setattr(generators, "_import", interrupted)
        assert generate()["status"] == "failed"
        monkeypatch.setattr(generators, "_import", original)
        assert generate()["status"] == "completed"
    assert len(generator_backend[0]) == 1


@pytest.mark.parametrize("overhead", [30000, 100000])
def test_request_132_maker_returns_only_the_generated_final_head(
        generator_repo, generator_backend, tmp_path, monkeypatch, overhead):
    from langchain_core.language_models.fake_chat_models import FakeMessagesListChatModel
    from langchain_core.messages import AIMessage
    from tl3_workers import runtime
    from tl3_workers.contracts import validate
    for name in ("socket_support.py", "test_short_socket.py"):
        (generator_repo / "tests" / name).unlink()
    (generator_repo / "tests/test_wait.py").write_text("from socket import socket\n")
    edits = [
        ("tests/socket_support.py", "def short_socket(): return 'short'\n"),
        ("tests/test_short_socket.py", "from tests.socket_support import short_socket\ndef test_short(): assert short_socket() == 'short'\n"),
        ("tests/test_wait.py", "from tests.socket_support import short_socket\n"),
    ]
    turns = [AIMessage(content="", tool_calls=[{"name": "write", "id": str(i),
              "args": {"path": path, "content": content}}]) for i, (path, content) in enumerate(edits)]
    turns.append(AIMessage(content=json.dumps({"kind": "done", "self_check": [
        {"goal_id": "socket", "status": "met", "evidence": "Imports use shared support."}]})))
    monkeypatch.setattr(runtime, "create_model", lambda request: FakeMessagesListChatModel(responses=turns))
    request = {"protocol": 1, "role": "maker", "worktree": str(generator_repo),
               "operation": {"node": 1, "generation": 0, "kind": "make"},
               "inputs": {"brief": "Request 132: share short Unix socket support and migrate test imports.",
                          "goals": [{"id": "socket", "description": "Use shared test support."}],
                          "plan": {"files": [path for path, _ in edits] + ["tests/BUILD.bazel"],
                                   "checks": [{'proof': {'subjects': [], 'observer_support': [], 'red_message': 'expected regression'}, "name": "support", "kind": "demonstrating",
                                               "tests": ["tests/test_short_socket.py::test_short"],
                                               "goal_ids": ["socket"]}]},
                          "forecast": {"fit_tokens": 100000, "context_tokens": 150000,
                                       "overhead_tokens": {"work": overhead},
                                       "generated_files": ["**/BUILD.bazel"],
                                       "generator": "python3 generate.py", "generator_available": True},
                          "attempt": 1, "round": 0, "context_size": 150000,
                          "tests": {"image": IMAGE, "execution": dict(FLUX_EXECUTION)}}}
    request["inputs"]["footprint"] = request["inputs"]["plan"]["files"]
    validate("maker-input", request["inputs"])
    events = []
    runtime.run(request, events.append)
    result = events[-1]["reply"]
    if overhead == 100000:
        assert result["kind"] == "cant_meet" and "final fit" in result["report"], result
        assert "head" not in result
        return
    assert result["kind"] == "done", result
    validate("maker-reply", result)
    tree = subprocess.check_output(["git", "rev-parse", result["head"] + "^{tree}"], cwd=generator_repo).decode().strip()
    assert result["generation_evidence"]["output_tree"] == tree
    assert result["generation_evidence"]["wait_reference"] == KEY
    subprocess.run(["python3", "generate.py", "--check"], cwd=generator_repo, check=True)
    assert "tests/socket_support.py:" in subprocess.check_output(
        ["git", "show", result["head"] + ":tests/BUILD.bazel"], cwd=generator_repo).decode()
    assert events[-1]["event"] == "reply"


def test_large_generated_output_uses_verified_artifact_without_entering_model_context(
        generator_repo, generator_backend, tmp_path):
    script = generator_repo / "generate.py"
    script.write_text(script.read_text() + "\nPath('tests/BUILD.bazel').write_text('# Generated by fixture\\n' + 'x' * 6_900_000)\n")
    tree, generate = configured_generator(generator_repo, tmp_path)
    with tree:
        result = generate()
        assert result["status"] == "completed", result
        assert len(json.dumps(result).encode()) < 16_384
        assert len(tree.read("tests/BUILD.bazel").encode()) < 3072
    assert (generator_repo / "tests/BUILD.bazel").stat().st_size > 6_900_000


def test_generator_tool_has_no_model_selected_command_or_arguments(generator_repo, tmp_path):
    tree, _ = configured_generator(generator_repo, tmp_path)
    with tree:
        tool = build_tools(tree, {"run_generator": "Run the configured admitted generator."})[0]
        assert tool.args == {}


def test_generator_image_command_cannot_be_shadowed_by_a_repository_file(tmp_path, monkeypatch):
    monkeypatch.setattr("tl3_harness.execution.scratch_directory", lambda: tmp_path)
    from tl3_harness import generator
    (tmp_path / "python3").write_text("#!/usr/bin/env bash\nexit 7\n")
    (tmp_path / "python3").chmod(0o755)
    command = generator.container_command(tmp_path, generator.Container(IMAGE),
                                          ["python3", "generate.py"], tmp_path / "cid")
    assert "--entrypoint=python3" in command
    assert command[-2:] == [IMAGE, "generate.py"]


@pytest.mark.parametrize("before_mode,after_mode", [(0o644, 0o755), (0o755, 0o644), (None, 0o755)])
def test_generator_rejects_mode_changes_in_action_and_artifact_validation(before_mode, after_mode):
    import base64
    from tl3_harness import generator
    name, raw = "tests/BUILD.bazel", b"generated\n"
    entry = {"sha256": hashlib.sha256(raw).hexdigest(), "bytes": len(raw), "mode": after_mode}
    before = {name: {**entry, "mode": before_mode}} if before_mode is not None else {}
    after = {name: entry}
    contract = {"command": "python3 generate.py", "generated_files": ["**/BUILD.bazel"],
                "protected_paths": [], "files": [name]}
    artifact = {"status": "completed", "exit_code": 0, "command": contract["command"],
                "argv": ["python3", "generate.py"], "input_manifest": generator.digest(before),
                "output_manifest": generator.digest(after),
                "outputs": [{"path": name, "entry": entry, "data": base64.b64encode(raw).decode()}]}
    with pytest.raises(ValueError, match="mode"):
        generator.changed(before, after, **{key: contract[key] for key in
                                           ("generated_files", "protected_paths", "files")})
    with pytest.raises(ValueError, match="mode"):
        generator.validate_artifact(artifact, before, contract)


@pytest.mark.parametrize("entrypoint", ["./generate.py", "tools/generate.py", "/checkout/generate.py",
                                       "/proc/self/cwd/generate.py"])
def test_generator_never_resolves_a_maker_written_shebang(tmp_path, entrypoint, monkeypatch):
    monkeypatch.setattr("tl3_harness.execution.scratch_directory", lambda: tmp_path)
    from tl3_harness import generator
    script = tmp_path / ("generate.py" if entrypoint.startswith("/") else entrypoint.removeprefix("./"))
    script.parent.mkdir(parents=True, exist_ok=True)
    script.write_text("#!/bin/sh -c 'printf changed > tests/BUILD.bazel'\n")
    script.chmod(0o755)
    with pytest.raises(ValueError, match="configured.*interpreter"):
        generator.container_command(tmp_path, generator.Container(IMAGE), [entrypoint], tmp_path / "cid")
    command = generator.container_command(tmp_path, generator.Container(IMAGE),
                                          ["python3", str(script.relative_to(tmp_path))], tmp_path / "cid")
    assert "--entrypoint=python3" in command
    assert command[-2:] == [IMAGE, str(script.relative_to(tmp_path))]


@pytest.mark.parametrize("cpus,mem_gb", [(1, 2), (4, 12)])
def test_generator_uses_repository_demand_and_native_thread_budget(
        generator_repo, generator_backend, tmp_path, cpus, mem_gb):
    tree, generate = configured_generator(generator_repo, tmp_path)
    tree.test_settings = flux_settings(cpus=cpus, mem_gb=mem_gb)
    with tree:
        assert generate()["status"] == "completed"
    [prepared] = generator_backend[0]
    assert prepared["document"]["cpus"] == cpus
    assert prepared["document"]["mem_gb"] == mem_gb
    assert tree.generation_receipt.get("budget", {}).get("cpus") == cpus
    assert tree.generation_receipt["budget"]["mem_gb"] == mem_gb


@pytest.mark.parametrize("ending", ["failed", "refused", "infra-lost", "cancelled"])
def test_flux_generator_without_a_completed_payload_fails_closed_and_keeps_its_binding(
        generator_repo, generator_backend, supported_fake_runtime, tmp_path, ending):
    supported_fake_runtime.ending = ending
    before = (generator_repo / "tests/BUILD.bazel").read_bytes()
    tree, generate = configured_generator(generator_repo, tmp_path)
    with tree:
        result = generate()
        assert result["status"] == "failed"
        assert result["wait_reference"] == KEY
        assert tree.generation_receipt == result
        assert json.loads(tree.generator_state.read_text())["phase"] == "submitted"
    assert (generator_repo / "tests/BUILD.bazel").read_bytes() == before
    assert supported_fake_runtime.submitted == [KEY]


@pytest.mark.parametrize("ending", ["failed", "refused", "infra-lost", "cancelled"])
def test_completed_generator_artifact_without_a_passed_ending_imports_nothing(
        generator_repo, generator_backend, supported_fake_runtime, tmp_path, monkeypatch, ending):
    from tl3_workers import flux_execution
    original = flux_execution.execute

    def execute(plan, **kwargs):
        payload = original(plan, **kwargs)
        assert payload["artifact"]["status"] == "completed" and payload["receipt"]["ending"] == "passed"
        payload["receipt"]["ending"] = ending
        return payload
    monkeypatch.setattr(flux_execution, "execute", execute)
    before = (generator_repo / "tests/BUILD.bazel").read_bytes()
    tree, generate = configured_generator(generator_repo, tmp_path)
    with tree:
        result = generate()
        assert result["status"] == "failed"
        assert "ending" in result["message"]
        assert json.loads(tree.generator_state.read_text())["phase"] == "submitted"
    assert (generator_repo / "tests/BUILD.bazel").read_bytes() == before


def test_flux_generator_refuses_another_payload_protocol(
        generator_repo, generator_backend, supported_fake_runtime, tmp_path):
    supported_fake_runtime.job = lambda root, document: ("tests", {"status": "completed"}, [])
    tree, generate = configured_generator(generator_repo, tmp_path)
    with tree:
        result = generate()
    assert result["status"] == "failed"
    assert "different generator protocol" in result["message"]


@pytest.mark.parametrize("timeout", [120, 300, 600])
def test_generator_repository_deadline_reaches_inner_and_outer_budgets(
        generator_repo, generator_backend, tmp_path, timeout):
    tree, generate = configured_generator(generator_repo, tmp_path)
    tree.generator_timeout_s = timeout
    with tree:
        result = generate()
    assert result["status"] == "completed", result
    [prepared] = generator_backend[0]
    assert prepared["document"]["walltime_seconds"] == timeout + 60
    assert result.get("budget", {}).get("timeout_s") == timeout
    assert result["ending"] == "passed" and "outer" not in result, \
        "the backend's whole-run ending stays separate from harness stages"


@pytest.mark.parametrize("change", ["cpus", "mem_gb", "deadline"])
@pytest.mark.durability
def test_generator_budget_change_cannot_reuse_a_checkpoint(
        generator_repo, generator_backend, tmp_path, change):
    tree, generate = configured_generator(generator_repo, tmp_path)
    with tree:
        first = generate()
        assert first["status"] == "completed", first
        if change == "deadline":
            tree.generator_timeout_s = 300
        else:
            tree.test_settings = flux_settings(cpus=4 if change == "cpus" else 1,
                                               mem_gb=12 if change == "mem_gb" else 2)
        second = generate()
    assert second["status"] == "completed", second
    assert len(generator_backend[0]) == 2, "changed budgets must submit a fresh sealed action"
    assert first["identity"] != second["identity"]
    assert second["output_count"] == 0


@pytest.mark.parametrize("fail", [False, True])
def test_generator_trusted_phase_receipts_include_timeout_and_cleanup(
        generator_repo, generator_backend, tmp_path, monkeypatch, fail):
    import shutil
    from tl3_harness import generator
    staged = tmp_path / "staged"
    shutil.copytree(generator_repo, staged, ignore=shutil.ignore_patterns(".git"))
    if fail:
        def timeout(*args, **kwargs):
            raise subprocess.TimeoutExpired(args[0], kwargs["timeout"])
        monkeypatch.setattr(generator, "container_command", lambda *args: ["generator-fixture"])
        original = generator.subprocess.run
        def launch(args, **kwargs):
            return timeout(args, **kwargs) if args == ["generator-fixture"] else original(args, **kwargs)
        monkeypatch.setattr(generator.subprocess, "run", launch)
    result = generator.run(staged, generator.Container(IMAGE), "python3 generate.py",
                           ["**/BUILD.bazel"], [], ["tests/BUILD.bazel"])
    assert result["status"] == ("failed" if fail else "completed"), result
    assert "timings" in result, "trusted stages must survive both success and inner timeout"
    phases = result["timings"]
    for phase in ("preparation", "container", "validation", "cleanup"):
        assert phases[phase]["seconds"] >= 0
        assert phases[phase]["outcome"] in ("completed", "timeout", "skipped")
    assert phases["total_seconds"] >= sum(phases[p]["seconds"] for p in
                                              ("preparation", "container", "validation", "cleanup"))
    assert phases["container"]["scope"] == "startup_and_command"
    if fail:
        assert result["phase"] == "container"
        assert result["exit_code"] == 124
        assert phases["container"]["outcome"] == "timeout"
        assert phases["validation"]["outcome"] == "skipped"


def test_request_163_implicit_generator_failure_retains_safe_causal_receipt(
        generator_repo, generator_backend, tmp_path, monkeypatch):
    from langchain_core.language_models.fake_chat_models import FakeMessagesListChatModel
    from langchain_core.messages import AIMessage
    from tl3_workers import runtime
    from tl3_workers.contracts import validate
    from tl3_harness import generator
    original = generator.subprocess.run
    def launch(args, **kwargs):
        if args == ["python3", "generate.py"]:
            raise subprocess.TimeoutExpired(args, kwargs["timeout"])
        return original(args, **kwargs)
    monkeypatch.setattr(generator.subprocess, "run", launch)
    turn = AIMessage(content=json.dumps({"kind": "done", "self_check": [
        {"goal_id": "build", "status": "met", "evidence": "Model work finished."}]}))
    monkeypatch.setattr(runtime, "create_model", lambda request: FakeMessagesListChatModel(responses=[turn]))
    request = {"protocol": 1, "role": "maker", "worktree": str(generator_repo),
               "operation": {"node": 163, "generation": 0, "kind": "make"},
               "inputs": {"brief": "Regenerate BUILD files after model work.",
                          "goals": [{"id": "build", "description": "Update BUILD files."}],
                          "plan": {"files": ["tests/BUILD.bazel"], "checks": [
                              {'proof': {'subjects': [], 'observer_support': [], 'red_message': 'expected regression'}, "name": "build", "kind": "demonstrating",
                               "tests": ["tests/test_wait.py::test_wait"], "goal_ids": ["build"]}]},
                          "forecast": {"fit_tokens": 100000, "context_tokens": 150000,
                                       "overhead_tokens": {"work": 30000},
                                       "generated_files": ["**/BUILD.bazel"],
                                       "generator": "python3 generate.py", "generator_available": True,
                                       "generator_timeout_s": 300},
                          "attempt": 1, "round": 0, "context_size": 150000,
                          "tests": {"image": IMAGE, "execution": dict(FLUX_EXECUTION),
                                    "cpus": 4, "mem_gb": 12}}}
    request["inputs"]["footprint"] = request["inputs"]["plan"]["files"]
    events = []
    runtime.run(request, events.append)
    reply = events[-1]["reply"]
    assert reply["kind"] == "cant_meet" and "head" not in reply
    assert KEY in reply["report"], "implicit failure must keep the failed run identity"
    assert "b" * 64 in reply["report"], "implicit failure must keep the verified receipt"
    assert '"exit_code": 124' in reply["report"]
    # FX classify: a typed generator failure ends Failed natively, with no native exit or time;
    # that outer verdict stays distinct from the inner generator exit 124.
    assert '"outer": {"elapsed_s": null, "exit_code": null, "status": "failed"}' in reply["report"], \
        "the native outer verdict must remain distinct from the inner failure"
    assert '"timeout_s": 300' in reply["report"]
    assert "container" in reply["report"]
    [prepared] = generator_backend[0]
    assert prepared["document"]["cpus"] == 4
    assert prepared["document"]["mem_gb"] == 12
    assert prepared["document"]["walltime_seconds"] == 360
    assert len(generator_backend[0]) == 1, "fail_fast must not retry work"
    validate("maker-reply", reply)


@pytest.mark.parametrize("timeout", [0, -1, 601, True, "300", 1.5])
def test_generator_rejects_invalid_repository_deadline_before_submission(
        generator_repo, generator_backend, tmp_path, timeout):
    tree, generate = configured_generator(generator_repo, tmp_path)
    tree.generator_timeout_s = timeout
    with tree:
        result = generate()
    assert result["status"] == "failed", result
    assert not generator_backend[0], "invalid trusted input must fail before submission"


@pytest.mark.durability
def test_failed_inner_generator_receipt_is_journaled_without_work_retry(
        generator_repo, generator_backend, tmp_path):
    (generator_repo / "generate.py").write_text("print('PRIVATE_DIAGNOSTIC')\nraise SystemExit(124)\n")
    tree, generate = configured_generator(generator_repo, tmp_path)
    with tree:
        result = generate()
        assert result["status"] == "failed", result
        assert "PRIVATE_DIAGNOSTIC" not in json.dumps(result)
        assert tree.generator_checkpoint.get("receipt") == result
        assert tree.generation_receipt == result
        assert generate() == result
    assert len(generator_backend[0]) == 1


@pytest.mark.parametrize("phase", ["preparation", "validation", "cleanup"])
def test_generator_stage_timeout_is_attributed_without_losing_inner_verdict(
        generator_repo, generator_backend, tmp_path, monkeypatch, phase):
    import shutil
    from tl3_harness import generator
    staged = tmp_path / "staged"
    shutil.copytree(generator_repo, staged, ignore=shutil.ignore_patterns(".git"))
    original = generator.manifest
    calls = 0
    def manifest(root):
        nonlocal calls
        calls += 1
        if (phase == "preparation" and calls == 1) or (phase == "validation" and calls == 2):
            raise generator.StageTimeout(phase)
        return original(root)
    monkeypatch.setattr(generator, "manifest", manifest)
    if phase == "cleanup":
        original_cleanup = generator.tempfile.TemporaryDirectory.cleanup
        def cleanup(self):
            original_cleanup(self)
            raise generator.StageTimeout("cleanup")
        monkeypatch.setattr(generator.tempfile.TemporaryDirectory, "cleanup", cleanup)
    result = generator.run(staged, generator.Container(IMAGE), "python3 generate.py",
                           ["**/BUILD.bazel"], [], ["tests/BUILD.bazel"])
    assert result["status"] == "failed", result
    assert result["phase"] == phase
    assert result["timings"][phase]["outcome"] == "timeout"
    assert result["timings"]["total_seconds"] >= result["timings"][phase]["seconds"]
    assert result["exit_code"] == (0 if phase == "cleanup" else 124)


def test_generator_still_requires_pb_admission_before_any_work(tmp_path, monkeypatch):
    from tl3_harness import generator
    for name in ("PRISMABUILD_CONTAINER_OWNER", "PRISMABUILD_CONTAINER_MARKER",
                 "PRISMABUILD_ACTION_PROGRESS_PATH", "PRISMABUILD_ACTION_PROGRESS_TOKEN"):
        monkeypatch.delenv(name, raising=False)
    with pytest.raises(RuntimeError, match="admitted PrismaBuild"):
        generator.run(tmp_path, generator.Container(IMAGE), "python3 generate.py",
                      ["**/BUILD.bazel"], [], ["tests/BUILD.bazel"])


# FX-GEN-PREP 80ddef3e: the real generator/collector's typed preparation failure.
FX_FAILURE = json.loads((Path(__file__).with_name("fixtures") / "fx-gen-prep"
                         / "preparation-failure-artifact.json").read_bytes())


def typed_failure_generator(generator_repo, tmp_path, fake, artifact=None):
    """Configure the generator the FX artifact was produced for and return its document."""
    from tl3_workers.tests_tool import TestSettings
    tree, generate = configured_generator(generator_repo, tmp_path)
    tree.generator = FX_FAILURE["command"]
    tree.generator_timeout_s = FX_FAILURE["budget"]["timeout_s"]
    tree.test_settings = TestSettings(FX_FAILURE["identity"]["image"], cpus=FX_FAILURE["budget"]["cpus"],
                                      mem_gb=FX_FAILURE["budget"]["mem_gb"], execution=dict(FLUX_EXECUTION))
    document = deepcopy(FX_FAILURE if artifact is None else artifact)
    fake.job = lambda root, prepared: ("generator", deepcopy(document), [])
    return tree, generate, document


@pytest.mark.durability
def test_typed_failed_generator_preparation_capture(generator_repo, generator_backend, supported_fake_runtime,
                                                    tmp_path):
    before = (generator_repo / "tests/BUILD.bazel").read_bytes()
    tree, generate, document = typed_failure_generator(generator_repo, tmp_path, supported_fake_runtime)
    supported_fake_runtime.ending = "failed"
    with tree:
        result = generate()
        assert result["status"] == "failed", result
        assert result["ending"] == "failed"
        assert result["outer"] == {"status": "failed", "exit_code": None, "elapsed_s": None}
        assert result["inner"] == {"status": "failed", "exit_code": None}
        assert result["phase"] == "preparation" and result["exit_code"] is None
        for name in ("command", "argv", "identity", "budget", "timings", "message"):
            assert result[name] == document[name], name
        assert result["backend"] == "flux" and result["wait_reference"] == KEY
        assert result["run_binding"] == run_binding(KEY) and result["receipt_sha256"] == "b" * 64
        assert result["artifact_sha256"] == hashlib.sha256(json.dumps(document).encode()).hexdigest()
        assert "cause" not in result, "the bounded cause stays in the verified artifact"
        assert not {"output_tree", "output_manifest", "output_count", "input_tree"} & set(result)
        assert tree.generation_receipt == result
        checkpoint = json.loads(tree.generator_state.read_text())
        assert checkpoint["phase"] == "failed" and checkpoint["receipt"] == result
        assert generate() == result, "a recorded failure is returned again, never resubmitted"
    assert supported_fake_runtime.submitted == [KEY] and supported_fake_runtime.attached == [KEY]
    assert (generator_repo / "tests/BUILD.bazel").read_bytes() == before


@pytest.mark.parametrize("case", ["passed_ending", "refused", "infra-lost", "cancelled", "unknown", "digest",
                                  "wrong_kind", "wrong_binding", "identity", "completed_with_failed_ending"])
def test_typed_failed_generator_preparation_refusals_keep_the_binding(
        generator_repo, generator_backend, supported_fake_runtime, tmp_path, case):
    artifact = deepcopy(FX_FAILURE)
    if case == "identity":
        artifact["identity"]["image"] = "sha256:" + "9" * 64
    if case == "completed_with_failed_ending":
        artifact["status"] = "completed"
    before = (generator_repo / "tests/BUILD.bazel").read_bytes()
    tree, generate, _ = typed_failure_generator(generator_repo, tmp_path, supported_fake_runtime, artifact)
    supported_fake_runtime.ending = {"passed_ending": "passed", "refused": "refused", "infra-lost": "infra-lost",
                                     "cancelled": "cancelled"}.get(case, "failed")
    if case == "digest":
        def corrupt(payload):
            path = Path(payload["artifact"]["path"])
            path.write_bytes(path.read_bytes() + b" ")
        supported_fake_runtime.on_payload = corrupt
    if case == "wrong_kind":
        supported_fake_runtime.job = lambda root, prepared: ("tests", deepcopy(artifact), [])
    if case == "unknown":
        supported_fake_runtime.observation = lambda key: {"kind": "unknown"}
    if case == "wrong_binding":
        # Another run's authoritative binding must never stand behind this wait reference.
        other = "worker:" + "e" * 64
        supported_fake_runtime.observation = lambda key: {
            "kind": "terminal", "binding": run_binding(other),
            "terminal": {"ending": "failed", "reason": "exited", "receipt": {"sha256": "b" * 64}}}
    with tree:
        result = generate()
        assert result["status"] == "failed", result
        assert "inner" not in result and "ending" not in result, "no typed capture from a refused payload"
        assert result["wait_reference"] == KEY
        assert json.loads(tree.generator_state.read_text())["phase"] == "submitted", "the binding is retained"
    assert supported_fake_runtime.submitted == [KEY]
    assert (generator_repo / "tests/BUILD.bazel").read_bytes() == before


@pytest.mark.parametrize("overhead", [30000])
def test_typed_failed_generator_preparation_admission_stays_failed(
        generator_repo, generator_backend, supported_fake_runtime, tmp_path, monkeypatch, overhead):
    from langchain_core.language_models.fake_chat_models import FakeMessagesListChatModel
    from langchain_core.messages import AIMessage
    from tl3_workers import runtime
    from tl3_workers.contracts import validate
    supported_fake_runtime.job = lambda root, prepared: ("generator", deepcopy(FX_FAILURE), [])
    supported_fake_runtime.ending = "failed"
    before = (generator_repo / "tests/BUILD.bazel").read_bytes()
    turn = AIMessage(content=json.dumps({"kind": "done", "self_check": [
        {"goal_id": "build", "status": "met", "evidence": "Model work finished."}]}))
    monkeypatch.setattr(runtime, "create_model", lambda request: FakeMessagesListChatModel(responses=[turn]))
    request = {"protocol": 1, "role": "maker", "worktree": str(generator_repo),
               "operation": {"node": 1574, "generation": 0, "kind": "make"},
               "inputs": {"brief": "Regenerate BUILD files after model work.",
                          "goals": [{"id": "build", "description": "Update BUILD files."}],
                          "plan": {"files": ["tests/BUILD.bazel"], "checks": [
                              {'proof': {'subjects': [], 'observer_support': [], 'red_message': 'expected regression'},
                               "name": "build", "kind": "demonstrating",
                               "tests": ["tests/test_wait.py::test_wait"], "goal_ids": ["build"]}]},
                          "forecast": {"fit_tokens": 100000, "context_tokens": 150000,
                                       "overhead_tokens": {"work": overhead},
                                       "generated_files": ["**/BUILD.bazel"],
                                       "generator": FX_FAILURE["command"], "generator_available": True,
                                       "generator_timeout_s": FX_FAILURE["budget"]["timeout_s"]},
                          "attempt": 1, "round": 0, "context_size": 150000,
                          "tests": {"image": FX_FAILURE["identity"]["image"], "execution": dict(FLUX_EXECUTION),
                                    "cpus": FX_FAILURE["budget"]["cpus"], "mem_gb": FX_FAILURE["budget"]["mem_gb"]}}}
    request["inputs"]["footprint"] = request["inputs"]["plan"]["files"]
    events = []
    runtime.run(request, events.append)
    reply = events[-1]["reply"]
    assert reply["kind"] == "cant_meet", reply
    assert "head" not in reply and "generation_evidence" not in reply, "a typed failure never completes generation"
    report = reply["report"]
    assert report.startswith("Generation failed before final head: " + FX_FAILURE["message"])
    for fragment in ('"ending": "failed"', '"phase": "preparation"', KEY, '"status": "failed"'):
        assert fragment in report, fragment
    assert '"status": "completed"' not in report and "Permission denied" not in report
    assert supported_fake_runtime.submitted == [KEY], "fail_fast must not retry work"
    assert (generator_repo / "tests/BUILD.bazel").read_bytes() == before
    validate("maker-reply", reply)
