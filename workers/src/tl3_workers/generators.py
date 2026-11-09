"""Submit the configured generator and import only its verified output artifact."""
import base64
import json
import os
from pathlib import Path
import subprocess
import tempfile

from tl3_harness import generator
from tl3_harness.scratch import environment as scratch_environment
from tl3_harness.supervision import ExecutionFailure
from tl3_workers.tests_tool import TestSettings, _git, _stage
from tl3_workers.session import save
from tl3_workers import flux_execution


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
        identity = {**contract, "image": settings.image, "scratch": scratch_environment(),
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
            if state.get("phase") in ("submitted", "importing"):
                raise ValueError("Generator contract changed during retained execution or interrupted import.")
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
            resumed = state.get('phase') == 'submitted'
            flux_execution.activation(settings)
            if not resumed:
                call, root = flux_execution.allocate(settings)
                tree = _stage(worktree, root, action="generator")
                if generator.manifest(root / "checkout") != current:
                    raise ValueError("Generator input tree changed while freezing its snapshot.")
                (root / "generator.json").write_text(json.dumps(contract, sort_keys=True))
                _git(root, "add", "--force", "--all")
                plan = flux_execution.prepare(settings, call, root, tree, contract,
                                              generator=True, walltime=selected['outer_timeout_s'])
                state = {"phase": "submitted", "contract": identity, "key": plan['key'],
                         "before": current, "input_tree": tree}
                record()
            key = state["key"]
            diagnostics.update(wait_reference=key, budget=selected, phase="outer", identity={
                "implementation": generator.IMPLEMENTATION, "image": settings.image, "budget": selected})
            if resumed:
                plan = flux_execution.resume(settings, key, state['input_tree'], contract)
            payload = flux_execution.execute(plan, resumed=resumed,
                                             emit=getattr(worktree, 'progress_emit', lambda event: None))
            if payload['kind'] != 'generator':
                raise ValueError('Flux returned a different generator protocol')
            artifact, receipt = payload['artifact'], payload['receipt']
            diagnostics.update(outer=receipt['outer'], phase='artifact_validation')
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
            if receipt["ending"] != "passed":
                # Only a passed run's outputs enter the worktree (maker-reply generation_receipt).
                raise ValueError(f"Generator run ending {receipt['ending']!r} is not passed; nothing was imported.")
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
        # A completed receipt carries the run's native ending once; failed-run
        # diagnostics above keep the backend's outer observation as recorded.
        receipt = {**{key: value for key, value in state["receipt"].items() if key != "outer"},
                   "status": "completed", "input_tree": state["input_tree"],
                   "output_tree": tree, "output_manifest": generator.digest(after),
                   "output_count": len(state["artifact"]["outputs"])}
        state.update(phase="applied", after=after, receipt=receipt)
        state.pop("artifact", None)
        record()
        worktree.generation_receipt = receipt
        return receipt

    return run
