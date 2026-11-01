"""Offline process replay for #1968 using controller-produced orientation.

Run inside PrismaBuild. Pass an inputs JSON file and its pinned checkout.
Only the model constructor is replaced, using the worker tests' recording path.
"""

import json
from pathlib import Path
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tests"))
import worker_process


INSPECT = '''
metrics_path = Path(sys.argv[1]).with_suffix('.metrics.json')
class InspectModel(FakeMessagesListChatModel):
    def bind_tools(self, tools, **kwargs):
        assert {tool.name for tool in tools} == {'list', 'read', 'glob', 'grep'}
        return self

    def _generate(self, messages, **kwargs):
        human = next(m.text for m in messages if m.type == 'human')
        repo = json.loads(human)['repo']
        assert set(repo) == {'readme', 'file_list', 'file_count', 'tree_summary'}
        metrics = {'input_bytes': len(human.encode()),
                   'prompt_bytes': sum(len(m.text.encode()) for m in messages if m.type in {'system', 'human'})}
        results = [m for m in messages if m.type == 'tool']
        if results:
            assert len(results) == 5
            assert all(m.status == 'success' for m in results), results
            assert any('--gpus' in m.text for m in results)
            metrics['tools'] = [m.name for m in results]
            metrics['tool_result_bytes'] = sum(len(m.text.encode()) for m in results)
        metrics_path.write_text(json.dumps(metrics))
        return super()._generate(messages, **kwargs)
'''


def main():
    inputs = json.loads(Path(sys.argv[1]).read_text())
    checkout = Path(sys.argv[2]).resolve()
    brief = {
        "kind": "brief",
        "brief": "Validate the complete Docker --gpus grammar before recording NVIDIA intent. "
                 "Inspect tools/fleet/docker and the Docker shim tests. Add the requested "
                 "malformed CSV, invalid count, unknown key and duplicate key coverage, "
                 "preserving accepted device and driver forms.",
        "goals": [{"description": text, "verification": "test"} for text in [
            "Reject malformed GPU options without recording intent.",
            "Preserve accepted quoted device and driver forms.",
            "Cover malformed CSV, invalid counts, unknown keys and duplicate keys.",
        ]],
        "memory_ids": [],
    }
    calls = [("list", {"path": "tools/fleet"}),
             ("glob", {"pattern": "*docker*", "path": "tests"}),
             ("grep", {"pattern": "--gpus", "path": "tools/fleet/docker"}),
             ("read", {"path": "tools/fleet/docker", "offset": 470, "limit": 100}),
             ("read", {"path": "tests/test_docker_shim_global_options.py", "limit": 100})]
    turns = [{"content": "", "tool_calls": [
        {"name": name, "args": args, "id": str(index), "type": "tool_call"}
        for index, (name, args) in enumerate(calls)
    ]}, {"content": json.dumps(brief)}]
    worker_process.LAUNCH = worker_process.LAUNCH.replace(
        'model = FakeMessagesListChatModel(', INSPECT + '\nmodel = InspectModel(')
    worker_process.LAUNCH = worker_process.LAUNCH.replace("def create_model(request):", """
original_run = runtime.run
def record_failures(request, emit):
    try:
        return original_run(request, emit)
    except Exception:
        import traceback
        traceback.print_exc()
        raise
runtime.run = record_failures
def create_model(request):""")
    with tempfile.TemporaryDirectory(prefix="tl3-ingest-replay-") as directory:
        root = Path(directory)
        request = {
            "protocol": 1, "operation": {"node": 0, "generation": 0, "kind": "ingest"},
            "role": "ingest", "worktree": str(checkout), "inputs": inputs,
            "model": {"base_url": "http://model.invalid/v1", "name": "recorded-model",
                      "effort": "high", "budget_tokens": 32000},
            "key_file": str(root / "never-read-key"),
        }
        result = worker_process.start_worker(root, request, turns)
        assert result.exit_code == 0, result.stderr
        assert result.events[-1]["reply"] == brief
        assert not Path(request["key_file"]).exists()
        metrics = json.loads((root / "scripted-model.metrics.json").read_text())
        assert len(metrics["tools"]) == 5
        print(json.dumps({"repository": "RobTand/" + sys.argv[3], "issue": 1968,
                          "model": "scripted recording; no network or credentials",
                          "exit_code": result.exit_code, **metrics}))


if __name__ == "__main__":
    main()
