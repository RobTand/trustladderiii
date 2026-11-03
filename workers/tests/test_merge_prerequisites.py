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
