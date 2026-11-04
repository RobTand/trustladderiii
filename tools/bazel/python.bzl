"""One target per test module, run the way the Light checks run it.

Gazelle writes the targets. This macro adds the runtime inputs Gazelle cannot
see, keyed by the test's package, so a new test module needs no hand edits.
"""

load("@rules_python//python:py_library.bzl", _py_library = "py_library")
load("@rules_python//python:py_test.bzl", "py_test")
load("@tl3_pip//:requirements.bzl", "requirement")
load("//tools/bazel:nonhermetic.bzl", "nonhermetic")
load("//tools/bazel:test_sizing.bzl", "test_size")

# Files a package's tests read at run time.
_PACKAGE_DATA = {
    "crates/controller": ["//harness/src/tl3_harness"],
    "deploy/ci-runners": ["//:workflows"],
    # TL3_DEPLOY_HELPER: the deployment tests drive the real helper binary.
    "deploy/dl380g10": ["//crates/config:sources", "//crates/controller:example_tl3-deploy"],
    "harness/tests": ["//harness:project_files", "//harness/tools:verify_disk_scratch"],
    "tools/ci": [
        "//:bazel_config",
        "//:workflows",
        "//crates/store:sources",
        "//harness:project_files",
        "//tools/bazel:sources",
        "//tools/bazel:workspace_build_files",
        "//workers:runtime_files",
    ],
    "workers/tests": ["//harness:project_files", "@o200k_base//file"],
}

# Project code that pytest imports from the project root, as Light checks do.
_PACKAGE_DEPS = {
    "harness/tests": ["//harness/src/tl3_harness", "@rules_python//python/runfiles"],
    "workers/tests": ["//workers/src"],
}

# Uncached single-shard seconds on main's post-merge Gate at 8403a295e7
# (BuildBuddy d3eb5c2b), or on #732's own runs for targets it moved in-process.
# Bazel splits these modules into shards of about _SHARD_SECONDS each;
# python_runner.py selects each shard's tests. Update a figure when a target's
# measured time moves, and add any Python target that passes 30 s.
_MEASURED_SECONDS = {
    "harness/tests:test_review_fixes": 35,
    "harness/tests:test_selection_groups": 60,
    "harness/tests:test_test_tool": 35,
    "workers/tests:test_large_files": 35,
    "workers/tests:test_maker": 43,
    "workers/tests:test_maker_forecast": 36,
    "workers/tests:test_maker_recovery": 41,
    "workers/tests:test_pb_machine_records": 37,
    "workers/tests:test_pb_tests": 38,
    "workers/tests:test_pb_tests_base": 48,
    "workers/tests:test_pb_tests_candidates": 41,
    "workers/tests:test_pb_tests_forcing": 30,
    "workers/tests:test_pb_tests_rehearsal": 54,
    "workers/tests:test_pb_tests_runtime": 40,
    "workers/tests:test_planner_forecast": 60,
    "workers/tests:test_reviewer": 68,
    "workers/tests:test_tracing": 66,
    "workers/tests:test_workers": 85,
}

_SHARD_SECONDS = 20

def _shard_count(package, name):
    """Shards for a measured target; 0 keeps Bazel's unsharded default."""
    seconds = _MEASURED_SECONDS.get(package + ":" + name, 0)
    shards = (seconds + _SHARD_SECONDS - 1) // _SHARD_SECONDS
    return shards if shards > 1 else 0

def _unique_labels(labels):
    """Gazelle writes //a/b, the tables //a/b:b; Bazel rejects the duplicate."""
    unique = {}
    for label in labels:
        unique[str(native.package_relative_label(label))] = label
    return list(unique.values())

def tl3_py_test(name, srcs, main = None, deps = [], data = [], **kwargs):
    package = native.package_name()
    module = main or srcs[0]
    tags, exec_properties = nonhermetic(name)
    exec_properties.update(test_size(package, name))
    # Measured near or over the 300s moderate class in uncached solo remote runs:
    # machine records exceeded 300s (main and wave 5); classifier took 286s alone
    # (wave 6, invocation 9ec338b6) and timed out under Gate load.
    if package == "workers/tests" and name in ("test_pb_machine_records", "test_classifier"):
        kwargs["timeout"] = "long"
    py_test(
        name = name,
        main = "//tools/bazel:python_runner.py",
        srcs = srcs + ["//tools/bazel:python_runner.py", "//tools/bazel:runtime.py"],
        args = [package + "/" + module],
        deps = _unique_labels(deps + _PACKAGE_DEPS.get(package, []) + ["//tools/ci:child_env", requirement("pytest")]),
        data = data + _PACKAGE_DATA.get(package, []) + native.glob(
            ["**"],
            exclude = ["BUILD.bazel", "**/__pycache__/**"],
            allow_empty = True,
        ),
        tags = kwargs.pop("tags", []) + tags,
        exec_properties = exec_properties,
        shard_count = _shard_count(package, name),
        **kwargs
    )

def py_library(name, data = [], imports = [], **kwargs):
    # tl3_workers loads prompts and schemas next to its modules.
    if native.package_name() == "workers/src":
        data = data + ["//workers:runtime_files"]
        imports = imports + ["."]
    _py_library(name = name, data = data, imports = imports, **kwargs)
