import json
from pathlib import Path

import pytest

from tl3_harness.model import Check, Kind

VECTORS = json.loads((Path(__file__).resolve().parents[1] / "test-vectors/pytest-node-ids.json").read_text())


@pytest.mark.parametrize("vector", VECTORS, ids=lambda v: repr(v["node_id"]))
def test_harness_exact_node_ids(vector):
    if vector["valid"]:
        Check("vector", Kind.REPRODUCING, (vector["node_id"],))
    else:
        with pytest.raises(ValueError):
            Check("vector", Kind.REPRODUCING, (vector["node_id"],))
