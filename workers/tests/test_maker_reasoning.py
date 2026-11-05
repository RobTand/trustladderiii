# ruff: noqa: F811

from pathlib import Path
from worker_process import start_worker
from message_invariants import DONE as DONE
from message_invariants import git as git
from message_invariants import maker_request as maker_request
from message_invariants import tool_capable_process_model as tool_capable_process_model
from message_invariants import turn as turn
from message_invariants import write_turn as write_turn


def test_maker_continues_past_a_turn_that_only_reasoned_long(tmp_path, maker_request):
    maker_request["inputs"]["context_size"] = 100_000
    root = Path(maker_request["worktree"])
    result = start_worker(tmp_path, maker_request, [
        write_turn(input_tokens=60_000, output_tokens=40_000, reasoning=39_000),
        turn(DONE, input_tokens=62_000, output_tokens=30_000, reasoning=25_000),
    ])
    assert result.exit_code == 0, result.stderr
    assert result.events[-1]["reply"] == {**DONE, "head": git(root, "rev-parse", "HEAD")}
