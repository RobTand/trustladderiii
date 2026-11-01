"""One target per test module, run the way the Light checks run it.

Gazelle writes the targets. This macro adds the runtime inputs Gazelle cannot
see, keyed by the test's package, so a new test module needs no hand edits.
"""

load("@rules_python//python:py_library.bzl", _py_library = "py_library")
load("@rules_python//python:py_test.bzl", "py_test")
load("@tl3_pip//:requirements.bzl", "requirement")
load("//tools/bazel:nonhermetic.bzl", "nonhermetic")

# Files a package's tests read at run time.
_PACKAGE_DATA = {
    "crates/controller": ["//harness/src/tl3_harness"],
    "deploy/ci-runners": ["//:workflows"],
    # TL3_DEPLOY_HELPER: the deployment tests drive the real helper binary.
    "deploy/dl380g10": ["//crates/config:sources", "//crates/controller:example_tl3-deploy"],
    "harness/tests": ["//harness:project_files"],
    "tools/ci": [
        "//:bazel_config",
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
    "harness/tests": ["//harness/src/tl3_harness"],
    "workers/tests": ["//workers/src"],
}

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
    py_test(
        name = name,
        main = "//tools/bazel:python_runner.py",
        srcs = srcs + ["//tools/bazel:python_runner.py", "//tools/bazel:runtime.py"],
        args = [package + "/" + module],
        deps = _unique_labels(deps + _PACKAGE_DEPS.get(package, []) + [requirement("pytest")]),
        data = data + _PACKAGE_DATA.get(package, []) + native.glob(
            ["**"],
            exclude = ["BUILD.bazel", "**/__pycache__/**"],
            allow_empty = True,
        ),
        tags = kwargs.pop("tags", []) + tags,
        exec_properties = exec_properties,
        **kwargs
    )

def py_library(name, data = [], imports = [], **kwargs):
    # tl3_workers loads prompts and schemas next to its modules.
    if native.package_name() == "workers/src":
        data = data + ["//workers:runtime_files"]
        imports = imports + ["."]
    _py_library(name = name, data = data, imports = imports, **kwargs)
