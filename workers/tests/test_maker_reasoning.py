# ruff: noqa: F811

from pathlib import Path
from langchain_core.messages import AIMessage
import pytest
from tl3_workers.roles import maker
from worker_process import start_worker
from message_invariants import DONE as DONE
from message_invariants import NO_USAGE as NO_USAGE
from message_invariants import git as git
from message_invariants import maker_request as maker_request
from message_invariants import read_call as read_call
from message_invariants import reasoning_usage as reasoning_usage
from message_invariants import tool_capable_process_model as tool_capable_process_model
from message_invariants import turn as turn
from message_invariants import write_turn as write_turn


@pytest.mark.parametrize("usage", NO_USAGE, ids=["missing", "zero"])
@pytest.mark.parametrize("characters,expected", [(2547, False), (2548, True), (2550, True)])
def test_usage_estimate_rounds_up_before_applying_85_percent(usage, characters, expected):
    message = AIMessage(content="x", usage_metadata=usage)
    assert maker.out_of_room(message, 1000, characters - 1) is expected


@pytest.mark.parametrize("reasoning", [None, 39_900], ids=["no-details", "reasoning-details"])
@pytest.mark.parametrize("input_tokens,output_tokens", [
    (84_073, 36_544),  # issue #355: the 59.7-minute turn of request 87
    (88_000, 40_000),  # the next turn: 128,000 of 150,000 by the old sum, 88,300 resent
])
def test_reasoning_tokens_do_not_fill_the_context(input_tokens, output_tokens, reasoning):
    message = AIMessage(content="", tool_calls=read_call(),
                        usage_metadata=reasoning_usage(input_tokens, output_tokens, reasoning))
    assert maker.out_of_room(message, 150_000, 1) is False


@pytest.mark.parametrize("with_details", [True, False], ids=["reasoning-details", "no-details"])
def test_visible_output_still_counts_toward_the_context(with_details):
    # 48,000 visible tokens at three characters each, resent with the next prompt.
    visible = "x" * 144_000
    reasoning = 2_000 if with_details else None
    message = AIMessage(content=visible, usage_metadata=reasoning_usage(80_000, 50_000, reasoning))
    assert maker.out_of_room(message, 150_000, 1) is True


@pytest.mark.parametrize("output_tokens,expected", [(47_499, False), (47_500, True)])
def test_reasoning_details_leave_exactly_the_visible_tokens(output_tokens, expected):
    message = AIMessage(content="ok", usage_metadata=reasoning_usage(80_000, 50_000, 50_000 - output_tokens))
    assert maker.out_of_room(message, 150_000, 1) is expected


def test_maker_continues_past_a_turn_that_only_reasoned_long(tmp_path, maker_request):
    maker_request["inputs"]["context_size"] = 100_000
    root = Path(maker_request["worktree"])
    result = start_worker(tmp_path, maker_request, [
        write_turn(input_tokens=60_000, output_tokens=40_000, reasoning=39_000),
        turn(DONE, input_tokens=62_000, output_tokens=30_000, reasoning=25_000),
    ])
    assert result.exit_code == 0, result.stderr
    assert result.events[-1]["reply"] == {**DONE, "head": git(root, "rev-parse", "HEAD")}
