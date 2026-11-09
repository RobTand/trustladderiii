"""Execution backend and checkout fixtures shared by worker contract tests."""
import hashlib
import json
from pathlib import Path
import subprocess

import pytest

from tl3_workers.tests_tool import TestSettings
from tl3_workers.tools import Worktree, build_tools


IMAGE = "sha256:" + "1" * 64
KEY = "worker:" + "a" * 64
FLUX_EXECUTION = {"backend": "flux", "profile": "/fixture/attempt/execution.json"}


def flux_settings(**resources):
    return TestSettings(IMAGE, execution=dict(FLUX_EXECUTION), **resources)


JOB_ALPHABET = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"


def flux_job_id(number):
    """A canonical Flux f58 job ID, as run_backend.rs canonical_job_id accepts."""
    digits = ""
    while True:
        number, digit = divmod(number, 58)
        digits = JOB_ALPHABET[digit] + digits
        if not number:
            return "f" + digits


def run_binding(key):
    """The RunBinding the Flux backend attaches for a worker run (run_backend.rs RunBinding).

    Fields follow BackendId and SubmissionIntent: Flux tag, instance generation,
    local:/// URI, canonical f58 job ID, worker-tools owner, the worker-<hex>
    correlation for this wait reference, the request digest and the flux-out directory.
    """
    correlation = key.replace("worker:", "worker-")
    instance, uri = "tl3-fixture", "local:///run/flux/fixture"
    return {"id": {"backend": "flux", "instance": instance, "uri": uri,
                   "job": flux_job_id(int(key.removeprefix("worker:")[:12], 16) + 1)},
            "intent": {"backend": "flux", "instance": instance, "uri": uri, "owner": "worker-tools",
                       "correlation": correlation,
                       "request_digest": hashlib.sha256(correlation.encode()).hexdigest(),
                       "output_dir": "/mnt/shared/flux-out/tl3/" + correlation}}


FAKES = {}


class FluxFake:
    """Replace only the Rust worker-execution control and shared CLI wire.

    The worker's real flux_execution.execute, payload blob checks and receipt
    mapping run. Each new submission runs ``job(root, document)`` once on its
    staged input root, as the admitted job would, and returns ``(kind, document,
    symptom_runs)``. Rust releases a payload only for a completed report: a passed
    run, or a trusted generator artifact reporting a typed failure with a failed
    ending. Any other ending or pairing refuses like the shared CLI's error frame.
    """

    def __init__(self, state, job):
        self.state, self.job = Path(state), job
        self.prepared, self.plans, self.roots, self.payloads, self.endings = [], {}, {}, {}, {}
        self.submitted, self.attached, self.resumed = [], [], []
        # None derives the terminal ending from the job's verified output, as Rust classify does.
        self.ending = None
        self.observation = None
        self.on_payload = None

    def install(self, monkeypatch):
        from tl3_workers import flux_execution
        monkeypatch.setattr(flux_execution, "control", self.control)
        monkeypatch.setattr(flux_execution, "_command", self.command)
        monkeypatch.setitem(FAKES, str(self.state), self)
        return self

    def control(self, document):
        op = document["op"]
        if op == "activation":
            return {}
        if op == "allocate":
            root = self.state / f"flux-input-{len(self.roots)}"
            root.mkdir(parents=True)
            self.roots[document["call"]] = root
            return {"root": str(root)}
        if op == "prepare":
            key = "worker:" + ("a" * 64 if not self.prepared else f"{len(self.prepared):064x}")
            self.prepared.append({"key": key, "root": self.roots[document["call"]], "document": document})
            self.plans[key] = {"key": key, "cli": "fixture-flux", "config": "fixture",
                               "bounds": {"payload_bytes": 32 * 1048576, "cli_envelope_bytes": 65536},
                               "submit": {"op": "submit", "key": key}, "attach": {"op": "attach", "key": key},
                               "owner": {"key": key}}
            return self.plans[key]
        if op == "resume":
            self.resumed.append(document["key"])
            if document["key"] not in self.plans:
                raise ValueError("fixture Flux refused an unknown retained binding")
            return self.plans[document["key"]]
        raise AssertionError(op)

    def _blob(self, document):
        raw = json.dumps(document).encode()
        path = self.state / f"flux-blob-{len(list(self.state.glob('flux-blob-*')))}.json"
        path.write_bytes(raw)
        return {"path": str(path), "bytes": len(raw), "sha256": hashlib.sha256(raw).hexdigest()}

    def _run(self, key):
        """Run the job once; return the ending Rust classifies its verified output as.

        Rust releases a payload only for a verified completed report. A trusted
        generator artifact reporting a typed failure ends Failed, never Passed;
        every other payload ends Passed (FX-GEN-PREP 80ddef3e: worker_protocol.rs
        generator_failed, evidence.rs classify).
        """
        if key not in self.payloads:
            [prepared] = [item for item in self.prepared if item["key"] == key]
            kind, document, symptoms = self.job(prepared["root"], prepared["document"])
            self.endings[key] = ("failed" if kind == "generator" and document.get("status") != "completed"
                                 else "passed")
            if kind == "generator":
                payload = {"kind": kind, "artifact": self._blob(document)}
            else:
                payload = {"kind": kind, "summary": self._blob(document),
                           "symptom_runs": [self._blob(run) for run in symptoms]}
            self.payloads[key] = payload
            if self.on_payload is not None:
                self.on_payload(payload)
        return self.endings[key]

    def command(self, plan, command):
        key, op = plan["key"], command["op"]
        if op == "submit":
            self.submitted.append(key)
            return {}
        if op == "attach":
            self.attached.append(key)
            if self.observation is not None:
                return {"observation": self.observation(key)}
            ending = self.ending if self.ending is not None else self._run(key)
            return {"observation": {"kind": "terminal", "binding": run_binding(key),
                                    "terminal": {"ending": ending, "reason": "exited",
                                                 "receipt": {"sha256": "b" * 64}}}}
        if op == "worker_payload":
            if command["terminal"]["ending"] not in ("passed", "failed"):
                raise ValueError("Shared execution retained an unknown or refused outcome")
            if command["terminal"]["ending"] != self._run(key):
                raise ValueError("Shared execution retained an unknown or refused outcome")
            return {"payload": self.payloads[key]}
        raise AssertionError(op)


def canned_tests_job(state):
    """Check the staged input like the admitted action, then return the canned summary."""
    state = Path(state)

    def job(root, document):
        root = Path(root)
        assert (root / "trusted/tl3_harness/runner.py").is_file()
        assert (root / "action.py").is_file()
        assert not (root / "checkout/.git").exists()
        (state / "submitted.json").write_text(json.dumps(document))
        (state / "tree.txt").write_text(subprocess.run(
            ["git", "write-tree", "--prefix=checkout/"], cwd=root,
            capture_output=True, text=True, check=True).stdout.strip())
        return "tests", json.loads((state / "response.json").read_text()), []
    return job


def action_payload(stdout):
    """Read the staged action's summary and symptom records, as the Rust worker protocol does."""
    from tl3_harness.test_tool import PREFIX
    summaries = [json.loads(line[len(PREFIX):]) for line in stdout.splitlines() if line.startswith(PREFIX)]
    symptoms = [json.loads(line.removeprefix("TL3_SYMPTOM_RUN ")) for line in stdout.splitlines()
                if line.startswith("TL3_SYMPTOM_RUN ")]
    assert len(summaries) == 1, stdout[-2000:]
    return "tests", summaries[0], symptoms


def generator_job(root, document):
    from tl3_harness import generator
    contract = json.loads((Path(root) / "generator.json").read_text())
    return "generator", generator.run(Path(root) / "checkout", generator.Container(document["image"]),
                                      **contract), []


@pytest.fixture(autouse=True)
def supported_fake_runtime(tmp_path, monkeypatch):
    """The admitted Flux backend, replaced only at its control and CLI wire."""
    return FluxFake(tmp_path / "tools", canned_tests_job(tmp_path / "tools")).install(monkeypatch)


@pytest.fixture
def submission(tmp_path, supported_fake_runtime):
    root = tmp_path / "worktree"
    root.mkdir()
    (root / "uncommitted.py").write_bytes(b"current dirty bytes\x00\xff")
    (root / ".gitignore").write_text("uncommitted.py\n")
    state = tmp_path / "tools"
    state.mkdir(exist_ok=True)
    job = canned_tests_job(state)

    def checked(staged, document):
        assert (Path(staged) / "checkout/uncommitted.py").read_bytes() == b"current dirty bytes\x00\xff"
        return job(staged, document)
    supported_fake_runtime.job = checked
    return root, state


def call(submission, response, tests=None):
    root, state = submission
    (state / "response.json").write_text(json.dumps(response))
    with Worktree(root) as tree:
        tree.test_settings = flux_settings()
        tool = build_tools(tree, {"run_tests": "Run worktree tests on the admitted test runner."})[0]
        assert set(tool.args) == {"tests", "base", "repeat", "parallel", "priority"}
        return tool.invoke({} if tests is None else {"tests": tests})


@pytest.fixture
def generator_repo(tmp_path):
    """A served repo with a git-index generator, like request 132's Bazel workflow."""
    root = tmp_path / "generated-repo"
    root.mkdir()
    script = '''import ast
from pathlib import Path
import subprocess
import sys
files = subprocess.check_output(["git", "ls-files", "-z"]).decode().split("\\0")
lines = []
for name in sorted(name for name in files if name.endswith(".py") and name != "generate.py"):
    imports = [node.module for node in ast.walk(ast.parse(Path(name).read_text()))
               if isinstance(node, ast.ImportFrom)]
    lines.append(name + ":" + ",".join(imports))
output = "# Generated by generate.py\\n" + "\\n".join(lines) + "\\n"
if "--check" in sys.argv:
    raise SystemExit(Path("tests/BUILD.bazel").read_text() != output)
Path("tests/BUILD.bazel").write_text(output)
'''
    (root / "generate.py").write_text(script)
    (root / "tests").mkdir()
    (root / "tests/test_wait.py").write_text("from socket import socket\n")
    (root / "tests/BUILD.bazel").write_text("# Generated by generate.py\ntests/test_wait.py:socket\n")
    for args in (("init", "--quiet", "--template="), ("add", "."),
                 ("-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid",
                  "commit", "--quiet", "-m", "Base")):
        subprocess.run(["git", *args], cwd=root, check=True, capture_output=True)
    (root / "tests/socket_support.py").write_text("def short_socket(): return 'short'\n")
    (root / "tests/test_short_socket.py").write_text("from tests.socket_support import short_socket\ndef test_short(): assert short_socket() == 'short'\n")
    (root / "tests/test_wait.py").write_text("from tests.socket_support import short_socket\n")
    return root


def configured_generator(root, tmp_path, *, protected=(), files=None):
    from tl3_workers.generators import make_generator_tool
    tree = Worktree(root)
    tree.test_checkout = root
    tree.test_settings = flux_settings()
    tree.generated_files = ["**/BUILD.bazel"]
    tree.generator = "python3 generate.py"
    tree.protected_paths = protected
    tree.generator_files = files or ["tests/BUILD.bazel", "tests/socket_support.py",
                                    "tests/test_short_socket.py", "tests/test_wait.py"]
    tree.generator_state = tmp_path / "generator-state.json"
    return tree, make_generator_tool(tree)
