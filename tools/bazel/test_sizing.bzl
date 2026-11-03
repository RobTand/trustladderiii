"""BuildBuddy reservations from execution receipts; see docs/bazel.md."""

def test_size(package, name, postgres = False):
    """Keep CPU predictions from replacing multi-process test reservations."""
    if package == "crates/controller":
        # #466: publishing bursts to 2.86 CPUs; max measured memory is 981 MB.
        # #483: worker shards burst to 2.24 CPUs and peak at 524 MB.
        cpu, memory = ("3", "1GB") if name == "test_test_workers" else ("4", "2GB")
    elif postgres:
        # Embedded Postgres: 0.91 CPU burst and 721 MB peak, plus 25% headroom.
        cpu, memory = "2", "1GB"
    elif package == "harness/tests":
        # Nested pytest: 0.99 CPU burst, 106 MB peak. Cover future nested suites too.
        cpu, memory = "2", "0.5GB"
    elif package == "workers/tests" and (name == "test_pb_tests" or name.startswith("test_pb_tests_")):
        # Nested harness runner: 1.02 CPU burst and 434 MB peak, plus 25% headroom.
        cpu, memory = "2", "1GB"
    else:
        return {}
    return {
        "EstimatedComputeUnits": "1",
        "EstimatedCPU": cpu,
        "EstimatedMemory": memory,
    }
