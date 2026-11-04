"""PB's prerequisite wording survives ingest as typed, quoted declarations."""

import pytest

from tl3_workers.contracts import validate


@pytest.mark.parametrize("quote,numbers", [
    ("Merge prerequisite: #2761. Wait for merged prerequisites and take a fresh main base.", [2761]),
    ("Merge prerequisites: #2779, #2772", [2779, 2772]),
])
def test_ingest_accepts_quoted_merge_prerequisites(quote, numbers):
    validate("ingest-reply", {
        "kind": "brief", "brief": "Implement the request after its prerequisites merge.",
        "goals": [{"description": "Implement the remaining work.", "verification": "test"}],
        "memory_ids": [],
        "merge_prerequisites": [
            {"kind": "issue", "number": number, "source_quote": quote}
            for number in numbers
        ],
    })


def test_prerequisite_quotes_are_checked_and_repairable():
    from tl3_workers.prerequisites import MergePrerequisiteError, check_prerequisites

    inputs = {"issue": {"title": "Request", "body": "Merge prerequisite: #12", "comments": []}}
    good = {"kind": "issue", "number": 12, "source_quote": "Merge prerequisite: #12"}
    check_prerequisites(inputs, {"merge_prerequisites": [good]})
    for prerequisites in [[{**good, "number": 1}], [good, good], [{**good, "source_quote": "#12 must merge first"}]]:
        with pytest.raises(MergePrerequisiteError, match="verbatim quote"):
            check_prerequisites(inputs, {"merge_prerequisites": prerequisites})


def test_prerequisite_failure_keeps_its_repairable_message():
    from tl3_workers.prerequisites import MergePrerequisiteError
    from tl3_workers.tracing import failure_tags

    assert failure_tags(MergePrerequisiteError()) == {
        "failure_class": "MergePrerequisiteError",
        "failure_message": MergePrerequisiteError.message,
    }


@pytest.mark.parametrize("prerequisites", [None, []])
def test_brief_without_prerequisites_needs_no_issue_evidence(prerequisites):
    from tl3_workers.roles.ingest import check_memory_ids

    reply = {"kind": "brief", "brief": "Implement the request.", "memory_ids": []}
    if prerequisites is not None:
        reply["merge_prerequisites"] = prerequisites
    check_memory_ids({}, reply)
    assert reply["brief"] == "Implement the request."
