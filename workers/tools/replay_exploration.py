"""Offline paired intake replay. Run inside PrismaBuild after exporting orientation.

Arguments: controller-produced inputs JSON, pinned checkout, issue number.
The recording uses real confined reads and the process test model seam. Token
counts use o200k_base on serialized prompts, not a live provider's accounting.
"""
import json
from pathlib import Path
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tests"))
import worker_process


INSPECT = '''
from dataclasses import replace
from tl3_workers.forecast import _encoding
from tl3_workers.roles import ROLES
from langchain_core.utils.function_calling import convert_to_openai_tool
encoding = _encoding()
metrics_path = Path(sys.argv[1]).with_suffix('.metrics.json')
metrics = []
offered_tools = []
class InspectModel(FakeMessagesListChatModel):
    def bind_tools(self, tools, **kwargs):
        offered_tools[:] = [convert_to_openai_tool(t) for t in tools]
        return self

    def _generate(self, messages, **kwargs):
        final = (len(metrics) == len(script) - 1
                 or any('budget is exhausted' in m.text for m in messages if m.type == 'human'))
        tools = [] if final else offered_tools
        payload = json.dumps({'messages': [m.model_dump(include={'type', 'content', 'tool_calls', 'tool_call_id', 'name'})
                                          for m in messages], 'tools': tools}, ensure_ascii=False)
        count = len(encoding.encode_ordinary(payload))
        metrics.append({'input_tokens': count, 'input_bytes': len(payload.encode()),
                        'tool_turn': not final,
                        'tool_result_bytes': sum(len(m.text.encode()) for m in messages if m.type == 'tool')})
        metrics_path.write_text(json.dumps(metrics))
        turn = script[-1] if final else script[len(metrics) - 1]
        self.responses = [AIMessage(**turn, usage_metadata={'input_tokens': count, 'output_tokens': 1,
                                                          'total_tokens': count + 1})]
        self.i = 0
        return super()._generate(messages, **kwargs)
if BASELINE:
    for name in ('ingest', 'audit'):
        ROLES[name] = replace(ROLES[name], exploration_budget=None)
'''


def replay(inputs, checkout, issue, role, baseline, omit_files=False):
    paths = (["src/prismabuild/_measurement_reservation.py", "tests/test_measurement_reservation_census_1419.py"]
             if issue == 1515 else ["tools/fleet/docker", "tests/test_docker_shim_global_options.py"])
    brief = {"kind": "brief", "brief": inputs["issue"]["title"],
             "goals": [{"description": inputs["issue"]["title"], "verification": "test"}],
             "memory_ids": [], "files": paths}
    if omit_files:
        del brief["files"]
    reply = brief if role == "ingest" else {"verdict": "agree", "objections": []}
    calls = [("list", {"path": "src/prismabuild" if issue == 1515 else "tools/fleet"}),
             ("glob", {"pattern": "*reservation*" if issue == 1515 else "*docker*", "path": "tests"}),
             ("grep", {"pattern": "locked_census" if issue == 1515 else "--gpus", "path": paths[0]}),
             *[("read", {"path": path, "offset": offset, "limit": 200})
               for offset in (0, 200, 400) for path in paths]]
    turns = [{"content": "", "tool_calls": [
        {"name": name, "args": args, "id": str(i), "type": "tool_call"}]} for i, (name, args) in enumerate(calls)]
    turns.append({"content": json.dumps(reply)})
    original = worker_process.LAUNCH
    worker_process.LAUNCH = original.replace('model = FakeMessagesListChatModel(',
        INSPECT.replace('BASELINE', repr(baseline)) + '\nmodel = InspectModel(')
    with tempfile.TemporaryDirectory(prefix="tl3-budget-replay-") as directory:
        root = Path(directory)
        request = {
            "protocol": 1, "operation": {"node": 0, "generation": 0, "kind": role},
            "role": role, "worktree": str(checkout), "inputs": json.loads(json.dumps(inputs)),
            "memory": {"summary_file": str(root / "memory.json")},
            "model": {"base_url": "http://model.invalid/v1", "name": "scripted-offline",
                      "effort": "high", "budget_tokens": 32000},
            "key_file": str(root / "never-read-key"),
        }
        if role == "audit":
            request["inputs"]["ingest"] = brief
        else:
            request["inputs"]["forecast"] = {"fit_tokens": 100_000, "context_tokens": 200_000,
                                              "overhead_tokens": {"prompt": 0}}
        result = worker_process.start_worker(root, request, turns)
        assert not Path(request["key_file"]).exists()
        metrics = json.loads((root / "scripted-model.metrics.json").read_text())
        failure = json.loads((root / "memory.json").read_text())["reason"]
        if omit_files:
            assert result.exit_code == 1
            assert failure["failure_message"] == "sized briefs must declare their root files"
        else:
            assert result.exit_code == 0, result.stderr
            assert result.events[-1]["event"] == "reply"
        report = {"issue": issue, "role": role, "baseline": baseline, "omit_files": omit_files,
                  "exit_code": result.exit_code, "input_tokens": sum(m["input_tokens"] for m in metrics),
                  "largest_context": max(m["input_tokens"] for m in metrics),
                  "model_turns": len(metrics), "tool_turns": sum(m["tool_turn"] for m in metrics),
                  "max_tool_result_bytes": max(m["tool_result_bytes"] for m in metrics),
                  "failure": failure}
        print(json.dumps(report), flush=True)
    worker_process.LAUNCH = original
    return report


def main():
    inputs = json.loads(Path(sys.argv[1]).read_text())
    checkout = Path(sys.argv[2]).resolve()
    issue = int(sys.argv[3])
    fixture = Path(__file__).resolve().parents[1] / "tests/fixtures" / f"prismabuild-{issue}.json"
    source = json.loads(fixture.read_text())
    inputs["issue"] = {"number": issue, "title": source["title"], "body": source["body"],
                       "author": "requester", "comments": []}
    for role in ("ingest", "audit"):
        before = replay(inputs, checkout, issue, role, True)
        after = replay(inputs, checkout, issue, role, False)
        assert after["tool_turns"] <= 4
        assert after["input_tokens"] <= 300_000
        assert after["input_tokens"] < before["input_tokens"]
        assert after["max_tool_result_bytes"] <= 32_768
    if issue == 1515:
        replay(inputs, checkout, issue, "ingest", False, omit_files=True)


if __name__ == "__main__":
    main()
