"""Temporary base-commit oracle for #780. Remove after integration ships.

The four .txt files are verbatim copies from the red commit's rust-fv base;
see git log for this directory's red commit and its exact parent.
"""
import json
import os
from pathlib import Path
import subprocess
import sys
import types
from unittest.mock import patch

from langchain_core.messages import AIMessage, HumanMessage, ToolMessage


def reference(name):
    module = types.ModuleType("t780_reference_" + name)
    sys.modules[module.__name__] = module
    exec(compile(Path(__file__).with_name(name + ".txt").read_text(), name + ".txt", "exec"), module.__dict__)
    return module


old = {name: reference(name) for name in ("exploration", "session", "forecast", "maker")}


def check(op, **facts):
    result = subprocess.run([os.environ["TL3_BINARY"], "check-budget"],
                            input=json.dumps({"kind": op, **facts}), text=True,
                            capture_output=True, check=True, timeout=5)
    return json.loads(result.stdout)["result"]


def facts_history(messages):
    return [{"kind": "ai", "calls": [c["id"] for c in m.tool_calls]} if isinstance(m, AIMessage)
            else {"kind": "tool", "id": m.tool_call_id} if isinstance(m, ToolMessage)
            else {"kind": "other"} for m in messages]


def run(mode, schedules):
    if mode == "shrinking_probe":
        assert all(n <= 3 for schedule in schedules for n in schedule), schedules
        return
    if mode == "io_checkpoint_mutants":
        import inspect
        import pytest
        from schedule import compare
        from tl3_workers import session
        from tl3_workers.roles import maker
        source = inspect.getsource(session.save)
        for omitted in ["json.dump(value, handle, ensure_ascii=False, allow_nan=False)",
                        "handle.flush()", "os.fsync(handle.fileno())", "os.replace(temporary, path)"]:
            namespace = {}
            exec(compile(source.replace(omitted, "pass"), "checkpoint_mutant", "exec"), session.__dict__, namespace)
            with patch.object(maker, "save", namespace["save"]):
                with pytest.raises(AssertionError):
                    compare(old, [[0, 1, 2]])
        return
    if mode == "io_schedule":
        from schedule import compare
        compare(old, schedules[:4])
        return
    if mode == "cutoff":
        from tl3_workers import session
        for counts in [(100, 20, 10), (0, 20, None), (0, 20, 30)]:
            outcomes = []
            for module in [old["session"], session]:
                calls = []
                values = iter([0, 3000])
                def characters(message):
                    calls.append("message_characters")
                    return next(values)
                message = AIMessage(content="", usage_metadata={
                    "input_tokens": counts[0], "output_tokens": counts[1],
                    "total_tokens": counts[0]+counts[1],
                    "output_token_details": {} if counts[2] is None else {"reasoning": counts[2]}})
                with patch.object(module, "_message_characters", characters):
                    result = module.out_of_room(message, 1000, 0)
                outcomes.append((calls, result))
            assert outcomes[0] == outcomes[1], (counts, outcomes)
    for schedule in schedules:
        for number in schedule:
            n = number
            if mode == "diagnostics":
                from tl3_workers.forecast import execution_forecast
                from tl3_workers.roles.planner import execution_budget_details
                for config in [{}, {"generator": ["generate"], "generated_files": ["b"]}]:
                    expected = old["forecast"].execution_forecast(["a", "b"], n % 10, exploration=n % 20, config=config)
                    actual = execution_forecast(["a", "b"], n % 10, exploration=n % 20, config=config)
                    assert execution_budget_details(actual).encode() == execution_budget_details(expected).encode()
            elif mode == "forecast":
                files = {"a": n, "b": n // 7}
                config = {"overhead_tokens": {"making": 4000}, "fit_tokens": 10000, "context_tokens": 15000}
                expected = old["forecast"].forecast_fit("héllo", files, config, problems=range(n % 3))
                assert check("forecast", brief_tokens=6, file_tokens=files, overhead_tokens=4000,
                             fit_tokens=10000, context_tokens=15000, problems=n % 3) == expected
            elif mode == "execution":
                from tl3_workers.roles.maker import ROLE
                expected = old["forecast"].execution_forecast(["a", "b"], n % 10, declared=n % 50,
                                                            exploration=None if n % 2 else n % 20)
                assert check("execution", files=2, generated=0, checks=n % 10, declared=n % 50,
                             exploration=None if n % 2 else n % 20, generator=False,
                             tool_turns=ROLE.exploration_budget.tool_turns,
                             finalization_turns=ROLE.exploration_budget.finalization_turns) == expected
            elif mode == "cutoff":
                message = AIMessage(content="visible café", usage_metadata={
                    "input_tokens": n, "output_tokens": n // 2, "total_tokens": n + n // 2,
                    "output_token_details": {"reasoning": n // 3}})
                expected = old["session"].out_of_room(message, 50000, 123)
                assert check("cutoff", input=n, output=n // 2, reasoning=n // 3,
                             message_characters=12, prompt_characters=123, context=50000) == expected
            elif mode == "charge":
                weights = old["exploration"].CacheWeights()
                details = {"cache_read": n // 2, "cache_creation": n // 7}
                expected = weights.charge(n, details)
                assert check("charge", tokens=n, read=n // 2, creation=n // 7,
                             weights=[1.0, 1.25, 0.1]) == expected
                forms = ["0", "-0", "0.0", "-0.0", "0e0", "0E+99", "-0.00e-123", "0e-999",
                         "1e-324", "-1e-324", "1", "-1", "1.0", "-1.0", "1e0", "-1E+99", "0.01",
                         "5e-324", "-5e-324", "1e-320", "1.7976931348623157e308",
                         "18446744073709551615", "18446744073709551616"]
                for field in ["cache_read", "cache_creation"]:
                    usage = {"input_tokens": 1, "input_token_details": {field: json.loads(forms[n % len(forms)])}}
                    original = old["exploration"].ExplorationMiddleware(old["exploration"].ExplorationBudget(), "maker")
                    original._account(types.SimpleNamespace(result=[AIMessage.model_construct(
                        content="", usage_metadata=usage)]), 1)
                    result = check("account", usage=[usage], bound=1, weights=[1.0, 1.25, 0.1],
                                   spent=0, turns=0, no_progress=0, final=False, repairing=False)
                    assert result["cache_active"] == original.cache_active
            elif mode == "history":
                messages = [HumanMessage(content="inputs"), AIMessage(content="", tool_calls=[
                    {"name": "read", "args": {}, "id": str(i)} for i in range(n % 4)]),
                    *[ToolMessage(content="result", tool_call_id=str(i)) for i in range(n % 3)]]
                expected_pending = [c["id"] for c in old["maker"]._pending_calls(messages, 0)]
                kept, interrupted = old["maker"]._resumable(messages, 0)
                assert check("history", messages=facts_history(messages), start=0) == {
                    "pending": expected_pending, "end": len(kept), "interrupted": interrupted}
            elif mode == "budget_reply":
                assert check("reply", done=bool(n % 2), unfinished=bool(n % 3)) == (bool(n % 2) and bool(n % 3))
            elif mode == "allowance":
                b = old["exploration"].ExplorationBudget(tool_turns=64, finalization_turns=8)
                a = old["exploration"].ExplorationMiddleware(b, "maker")
                a.turns, a.spent = n % 70, n * 4
                expected = a.remaining()
                assert check("remaining", maker=True, turns=a.turns, spent=a.spent,
                             tool_turns=64, input_tokens=300000, finalization_turns=8,
                             no_progress_window=0, no_progress_turns=0, last_progress_kind=None,
                             implementation_files=False, implementation_started=False,
                             phase=None, exhaustion=None, finalizing=False, final=False) == expected
                from tl3_workers import exploration
                pair = [module.ExplorationMiddleware(module.ExplorationBudget(no_progress_window=n % 3),
                                                     "maker" if n & 1 else "planner")
                        for module in [old["exploration"], exploration]]
                for allowance in pair:
                    allowance.finalizing, allowance.final, allowance.repairing = bool(n & 2), bool(n & 4), bool(n & 8)
                for name in ["read", "grep", "glob", "run_tests", "write", "edit", "list", "diff", "custom"]:
                    request = types.SimpleNamespace(tool_call={"name": name, "args": {}, "id": name})
                    outputs = [allowance.wrap_tool_call(request, lambda _: ToolMessage(content="result", tool_call_id=name))
                               for allowance in pair]
                    assert outputs[0].model_dump() == outputs[1].model_dump()
                    for fresh in [False, True]:
                        for changed in [False, True]:
                            text = str((n, fresh, changed))
                            result = ({"matches": [text], "entries": [text], "status": text}
                                      if name in {"grep", "glob", "run_tests"} else text)
                            for _ in range(2):
                                for allowance in pair:
                                    allowance.no_progress_turns = 3
                                    allowance.observe_tool(name, {}, result, fresh=fresh, changed=changed)
                                assert pair[0].state() == pair[1].state(), (name, fresh, changed, n)
            elif mode == "compaction":
                from tl3_workers import exploration
                from langchain_core.messages import SystemMessage
                transcript = [SystemMessage(content="system"), HumanMessage(content="inputs")]
                for i in range(1 + n % 24):
                    transcript.extend([AIMessage(content=[{"type": "text", "text": "long café\\n\t\x00😀" * 1500}]
                        if n % 3 else "long café\\n" * 1500,
                        additional_kwargs={"tool_calls": [{"raw": "provider arguments" * 500}]} if n % 4 == 3 else {},
                        tool_calls=[{"name": "read", "id": str(i), "args": {
                            "z": "long argument\\t" * (1500 if n % 2 else 1), "a": "é"}}]),
                        ToolMessage(content="long tool message\\n" * 2000, tool_call_id=str(i)),
                        HumanMessage(content="Retain this instruction.")])
                expected = old["exploration"].retained(transcript, 1024)
                calls = []
                original = exploration.budget_check
                def measured(kind, **facts):
                    calls.append(kind)
                    return original(kind, **facts)
                with patch.object(exploration, "budget_check", measured):
                    actual = exploration.retained(transcript, 1024)
                assert json.dumps([m.model_dump() for m in actual], ensure_ascii=False).encode() == json.dumps(
                    [m.model_dump() for m in expected], ensure_ascii=False).encode()
                assert len(calls) <= 1, (len(transcript), calls)
                assert check("compaction", before=n, after=n // 2) == (n // 2 < n)
    print(mode, "parity schedules", len(schedules))


if "--server" in sys.argv:
    import contextlib
    import io
    import traceback
    for line in sys.stdin:
        try:
            with contextlib.redirect_stdout(io.StringIO()):
                run(sys.argv[1], [json.loads(line)])
            response = {"ok": True}
        except Exception:
            response = {"ok": False, "error": traceback.format_exc()}
        print(json.dumps(response), flush=True)
else:
    run(sys.argv[1], json.load(sys.stdin))
