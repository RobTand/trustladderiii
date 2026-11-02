"""Tests that cannot run hermetically, each with its reason.

The remote platform turns networking off. A test listed here as
requires-network gets it back. Keep this table short and keep the reasons true;
the PR that adds an entry says why the test cannot use a declared input.
"""

NONHERMETIC = {
    "//workers/tests:test_planner": struct(
        tags = ["requires-network"],
        reason = "Its session fixture downloads tiktoken's o200k_base into an empty cache on purpose.",
    ),
    "//workers/tests:test_sizing": struct(
        tags = ["requires-network"],
        reason = "Its session fixture downloads tiktoken's o200k_base into an empty cache on purpose.",
    ),
}

def nonhermetic(name):
    """Returns (tags, exec_properties) for the named test in this package."""
    entry = NONHERMETIC.get("//%s:%s" % (native.package_name(), name))
    if not entry:
        return [], {}
    network = {"dockerNetwork": "bridge"} if "requires-network" in entry.tags else {}
    return entry.tags, network
