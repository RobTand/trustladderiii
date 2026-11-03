"""Submit the configured generator and import only its verified output artifact."""
import base64
import hashlib
import json
import math
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile

from tl3_harness import generator
from tl3_harness.supervision import ExecutionFailure
from tl3_workers.pb_tests import TestSettings, _failure_receipt, _git, _json_command, _stage
from tl3_workers.session import save


def _elapsed(ending):
    elapsed = (ending.get("diagnostics") or {}).get("elapsed_s")
    return elapsed if type(elapsed) in (int, float) and math.isfinite(elapsed) and elapsed >= 0 else None


def _artifact(ending, key):
    if (ending["action_key"] != key or ending["status"] not in ("executed", "cache_hit")
            or ending["exit_code"] != 0 or ending["succeeded"] is not True):
        raise ValueError("Generator action did not succeed; inspect its PrismaBuild terminal record.")
    evidence = ending["result"]
    if evidence["verified"] is not True or not re.fullmatch("[0-9a-f]{64}", evidence["receipt_sha256"]):
        raise ValueError("Generator action has no verified PrismaBuild receipt.")
    log = evidence["stdout"]
    if type(log["bytes"]) is not int or not 0 < log["bytes"] <= generator.ARTIFACT_BYTES:
        raise ValueError("Generator artifact exceeds its limit.")
    with open(log["path"], "rb") as stream:
        raw = stream.read(generator.ARTIFACT_BYTES + 1)
    if len(raw) != log["bytes"] or hashlib.sha256(raw).hexdigest() != log["sha256"]:
        raise ValueError("Generator artifact digest mismatch.")
    records = [line[len(generator.PREFIX):] for line in raw.decode().splitlines()
               if line.startswith(generator.PREFIX)]
    if len(records) != 1:
        raise ValueError("Generator action returned no unique complete artifact.")
    artifact = json.loads(records[0])
    return artifact, {"action_key": key, "artifact_sha256": log["sha256"],
                      "receipt_sha256": evidence["receipt_sha256"], "exit_code": artifact["exit_code"],
                      "command": artifact.get("command"), "argv": artifact.get("argv"),
                      "identity": artifact.get("identity"), "budget": artifact.get("budget"),
                      "phase": artifact.get("phase"), "timings": artifact.get("timings"),
                      "inner": {"status": artifact["status"], "exit_code": artifact["exit_code"]},
                      "outer": {"status": ending["status"], "exit_code": ending["exit_code"],
                                "elapsed_s": _elapsed(ending)}}


def _import(worktree, outputs):
    # Journaled before the first write. Each file replacement is atomic; retry
    # accepts only the original or expected bytes, never an unrelated source edit.
    for output in outputs:
        parts = generator.path_parts(output["path"])
        with worktree._directory(parts[:-1], create=True) as parent:
            fd, name = None, ".tl3-generator-import"
            try:
                fd = os.open(name, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW,
                             output["entry"]["mode"], dir_fd=parent)
                with os.fdopen(fd, "wb") as stream:
                    fd = None
                    stream.write(base64.b64decode(output["data"], validate=True))
                    stream.flush()
                    os.fchmod(stream.fileno(), output["entry"]["mode"])
                    os.fsync(stream.fileno())
                os.replace(name, parts[-1], src_dir_fd=parent, dst_dir_fd=parent)
                os.fsync(parent)
            finally:
                if fd is not None:
                    os.close(fd)
                # An interrupted import may leave its temporary. Its name is
                # reserved below and recovered before taking a worktree manifest.
                try:
                    os.unlink(name, dir_fd=parent)
                except FileNotFoundError:
                    pass


def _recover_temporaries(worktree, state):
    if state.get("phase") != "importing":
        return
    for output in state["artifact"]["outputs"]:
        parts = generator.path_parts(output["path"])
        try:
            with worktree._directory(parts[:-1]) as parent:
                os.unlink(".tl3-generator-import", dir_fd=parent)
        except FileNotFoundError:
            pass


def make_generator_tool(worktree, *, restore_only=False):
    transient_state = getattr(worktree, "generator_checkpoint", {})
    diagnostics = {}

    def run() -> dict:
        """Regenerate configured outputs. There are no model-selected commands or arguments."""
        diagnostics.clear()
        try:
            return generate()
        except ExecutionFailure as failure:
            result = {"status": "failed", "message": failure.message(),
                      **diagnostics, "execution": failure.as_json()}
            worktree.generation_receipt = result
            return result
        except (OSError, RuntimeError, ValueError, KeyError, TypeError, subprocess.SubprocessError) as error:
            result = {"status": "failed", "message": str(error)[:1000], **diagnostics}
            worktree.generation_receipt = result
            return result

    def generate():
        settings = getattr(worktree, "test_settings", None)
        command = getattr(worktree, "generator", None)
        patterns = list(getattr(worktree, "generated_files", ()))
        if not isinstance(settings, TestSettings) or not command or not patterns:
            return {"status": "not_available", "message": "No admitted generator command/output contract was configured."}
        generator.configured_argv(command)
        selected = generator.budget(settings.cpus, settings.mem_gb,
                                    getattr(worktree, "generator_timeout_s", generator.TIMEOUT))
        checkout = worktree.test_checkout
        contract = {"command": command, "generated_files": patterns,
                    "protected_paths": list(getattr(worktree, "protected_paths", ())),
                    "files": list(worktree.generator_files), "timeout": selected["timeout_s"],
                    "cpus": selected["cpus"], "mem_gb": selected["mem_gb"]}
        identity = {**contract, "image": settings.image, "exec_scratch": settings.exec_scratch,
                    "implementation": generator.IMPLEMENTATION, "budget": selected}
        state_path = getattr(worktree, "generator_state", None)
        nonlocal transient_state
        state = json.loads(state_path.read_text()) if state_path and state_path.exists() else transient_state

        def record():
            nonlocal transient_state
            transient_state = state
            worktree.generator_checkpoint = state
            if state_path:
                save(state_path, state)

        if restore_only and (state.get("contract") != identity or state.get("phase") != "applied"):
            return {"status": "not_available", "message": "No completed generation checkpoint matches this contract."}
        if state.get("contract") != identity:
            if state.get("phase") == "importing":
                raise ValueError("Generator contract changed during an interrupted import.")
            state = {}
        if not restore_only:
            _recover_temporaries(worktree, state)
        current = generator.manifest(checkout)
        if any(Path(name).name == ".tl3-generator-import" for name in current):
            raise ValueError("Reserved generator import path exists in the worktree.")
        if state.get("phase") == "failed" and current == state["before"]:
            worktree.generation_receipt = state["receipt"]
            return state["receipt"]
        if state.get("phase") == "applied" and current == state["after"]:
            if restore_only and (state["receipt"].get("status") != "completed"
                                 or state["receipt"].get("output_manifest") != generator.digest(current)):
                return {"status": "not_available", "message": "Generation receipt does not match the current manifest."}
            worktree.generation_receipt = state["receipt"]
            return state["receipt"]
        if restore_only:
            return {"status": "not_available", "message": "Generation checkpoint is stale for the current worktree."}
        if state.get("phase") == "importing":
            after = generator.validate_artifact(state["artifact"], state["before"], contract)
            if any(current.get(name) not in (state["before"].get(name), after.get(name))
                   for name in current.keys() | state["before"].keys() | after.keys()):
                raise ValueError("Stale input tree during generator import; source work was preserved.")
        else:
            if state.get("phase") == "submitted" and current != state["before"]:
                raise ValueError("Generator action input tree is stale; restore the submitted input before reattachment.")
            if state.get("phase") != "submitted":
                with tempfile.TemporaryDirectory(prefix="tl3-generator-input-") as temporary:
                    root = Path(temporary)
                    tree = _stage(worktree, root, action="generator")
                    if generator.manifest(root / "checkout") != current:
                        raise ValueError("Generator input tree changed while freezing its snapshot.")
                    (root / "generator.json").write_text(json.dumps(contract, sort_keys=True))
                    _git(root, "add", "--force", "--all")
                    args = [sys.executable, str(Path(settings.tools_path) / "pbrun.py"),
                            "--cwd", str(root), "--anywhere", "--cpus", str(settings.cpus),
                            "--demand", f"mem_gb={settings.mem_gb}", "--container-image", settings.image,
                            "--timeout-s", str(selected["outer_timeout_s"]), "--progress-phase",
                            f"tests={selected['outer_timeout_s']}",
                            *[arg for name in ("OMP_NUM_THREADS", "MKL_NUM_THREADS", "OPENBLAS_NUM_THREADS")
                              for arg in ("--env", f"{name}={settings.cpus}")], "--detach", "--",
                            "python3", "-I", "-B", "action.py", settings.image]
                    if settings.exec_scratch:
                        args.append("--exec-scratch")
                    code, submitted = _json_command(args)
                    if code or submitted.get("schema") not in (
                            "prismabuild.submission.v1", "prismaquant.prismabuild.pbrun_detach.v1"):
                        raise ValueError("PrismaBuild refused generator submission.")
                    key = submitted["action_key"]
                    if not isinstance(key, str) or not re.fullmatch("[0-9a-f]{64}", key):
                        raise ValueError("PrismaBuild returned an invalid generator action key.")
                    state = {"phase": "submitted", "contract": identity, "key": key,
                             "before": current, "input_tree": tree}
                    record()
            key = state["key"]
            diagnostics.update(action_key=key, budget=selected, phase="outer", identity={
                "implementation": generator.IMPLEMENTATION, "image": settings.image, "budget": selected})
            while True:
                getattr(worktree, "progress_emit", lambda event: None)({
                    "event": "progress", "unit": "external_wait",
                    "wait": {"upstream": "prismabuild", "key": key}})
                code, waited = _json_command([sys.executable, str(Path(settings.tools_path) / "pbwait.py"),
                                              "--endings", "--until", "cleaned", key])
                if (waited.get("schema") != "prismabuild.endings.v1" or
                        type(waited.get("complete")) is not bool or len(waited["endings"]) != 1 or
                        waited["endings"][0]["action_key"] != key):
                    raise ValueError("PrismaBuild returned an invalid generator ending.")
                if waited["complete"]:
                    if code or not waited["endings"][0].get("succeeded"):
                        receipt = _failure_receipt(waited["endings"][0], settings)
                        result = {"status": "failed", "action_key": key, "receipts": [receipt],
                                  "message": "Generator action failed: " + json.dumps(receipt, sort_keys=True),
                                  "budget": selected, "phase": "outer", "identity": {
                                      "implementation": generator.IMPLEMENTATION, "image": settings.image,
                                      "budget": selected}, "outer": {**receipt, "elapsed_s": _elapsed(waited["endings"][0])},
                                  "inner": {"status": "unavailable", "exit_code": None}}
                        state.update(phase="failed", receipt=result)
                        record()
                        worktree.generation_receipt = result
                        return result
                    break
                if waited["endings"][0]["status"] not in ("waiting", "executed", "cache_hit"):
                    raise ValueError("PrismaBuild could not verify generator completion.")
            diagnostics.update(outer=_failure_receipt(waited["endings"][0], settings), phase="artifact_validation")
            artifact, receipt = _artifact(waited["endings"][0], key)
            if (artifact.get("command") != command or artifact.get("identity") != {
                    "implementation": generator.IMPLEMENTATION, "image": settings.image, "budget": selected}
                    or artifact.get("budget") != selected):
                raise ValueError("Generator artifact identity differs from its configured command/image/budget.")
            if artifact["status"] != "completed" or artifact["exit_code"] != 0:
                result = {**receipt, "status": "failed", "message": artifact["message"]}
                state.update(phase="failed", receipt=result)
                record()
                worktree.generation_receipt = result
                return result
            after = generator.validate_artifact(artifact, current, contract)
            if generator.manifest(checkout) != current:
                raise ValueError("Generator artifact input tree is stale; source work was preserved.")
            state.update(phase="importing", artifact=artifact, receipt=receipt)
            record()
        _import(worktree, state["artifact"]["outputs"])
        if generator.manifest(checkout) != after:
            raise ValueError("Generator import did not produce its verified output tree.")
        with tempfile.TemporaryDirectory(prefix="tl3-generated-tree-") as temporary:
            tree = _stage(worktree, Path(temporary))
        receipt = {**state["receipt"], "status": "completed", "input_tree": state["input_tree"],
                   "output_tree": tree, "output_manifest": generator.digest(after),
                   "output_count": len(state["artifact"]["outputs"])}
        state.update(phase="applied", after=after, receipt=receipt)
        state.pop("artifact", None)
        record()
        worktree.generation_receipt = receipt
        return receipt

    return run
