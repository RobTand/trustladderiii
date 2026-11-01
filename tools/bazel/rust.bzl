"""Cargo crates as Bazel targets, with the hosted gate's budgets and receipts.

crates/*/BUILD.bazel holds only facts from `cargo metadata`, written by
tools/bazel/generate.sh. The tables below hold what Cargo cannot say.

Each Cargo test binary builds as a manual rust_test, and a py_test runs it
through tools/ci/rust_gate.py, the same code the Light checks use.
"""

load("@crates//:defs.bzl", "aliases", "all_crate_deps")
load("@rules_python//python:py_test.bzl", "py_test")
load("@rules_rust//cargo:defs.bzl", "cargo_build_script")
load("@rules_rust//rust:defs.bzl", "rust_binary", "rust_library", "rust_test")
load("//tools/bazel:nonhermetic.bzl", "nonhermetic")
load("//tools/bazel:python.bzl", "tl3_py_test")

# Cargo tests read env!("CARGO_MANIFEST_DIR") and some require it absolute.
# rust_runner.py points this fixed path at the test's runfiles. Each test needs
# a private /tmp, which remote containers and Bazel's sandbox both provide.
WORKSPACE = "/tmp/tl3-workspace"

_POSTGRES = ["crates/controller", "crates/store"]

# Files that include_str! and build.rs read from outside the crate.
_COMPILE_DATA = {
    "crates/controller": [
        "//crates/config:sources",
        "//crates/github:sources",
        "//crates/mergify:sources",
        "//crates/store:sources",
        "//harness:project_files",
        "//workers:schemas",
        "//workers/tests:fixtures",
    ],
    "crates/github": ["//crates/mergify:sources"],
    "crates/mergify": ["//crates/github:sources"],
}
_BUILD_SCRIPT_DATA = {
    "crates/controller": ["//harness:project_files", "//workers:schemas", "//workers/tests:fixtures"],
}
_EXAMPLE_DATA = {
    "//deploy/dl380g10:helper.rs": ["//deploy/github-app:manifest.json"],
}

# rust_gate.py runs the controller's github binary as two timed groups.
_GATE_GROUPS = {"tl3-controller/github": ["flow", "notices"]}

# Controller tests start its binaries and the Python workers.
_CONTROLLER_BINARIES = [":tl3", ":tl3d"]
_CONTROLLER_RUNTIME = [
    "//crates/prismabuild:sources",
    "//deploy/sandbox:evidence",
    "//harness:project_files",
    "//tools/bazel:worker_python",
    "//workers:runtime_files",
    "//workers/tests:worker_process",
    "//workers/tools:scripts",
    "@o200k_base//file",
]

def tl3_rust_test(name, binary_id, gate_args, **kwargs):
    package = native.package_name()
    controller = package == "crates/controller"
    binaries = _CONTROLLER_BINARIES if controller else []
    env = {"CARGO_MANIFEST_DIR": WORKSPACE + "/" + package}
    for binary in binaries:
        env["CARGO_BIN_EXE_" + binary[1:]] = WORKSPACE + "/$(rootpath " + binary + ")"
    runtime = _CONTROLLER_RUNTIME if controller else []
    postgres = select({
        "@platforms//cpu:aarch64": ["@postgres_linux_arm64//:all"],
        "//conditions:default": ["@postgres_linux_amd64//:all"],
    }) if package in _POSTGRES else []
    tags, exec_properties = nonhermetic(name)
    if controller:
        # These binaries run Postgres, worker processes and git side by side.
        exec_properties = dict(exec_properties, EstimatedCPU = "4", EstimatedMemory = "8GB")
    runtime_data = kwargs.pop("data", [])
    harness = "libtest" if kwargs.get("use_libtest_harness", True) else "custom"
    rust_test(
        name = name + "_binary",
        tags = ["manual"],
        rustc_env = env,
        data = binaries + runtime_data,
        **kwargs
    )
    py_test(
        name = name,
        srcs = ["//tools/bazel:rust_runner.py", "//tools/bazel:runtime.py"],
        main = "//tools/bazel:rust_runner.py",
        args = [
            "$(rootpath :%s_binary)" % name,
            binary_id,
            harness,
            "$(rootpath //tools/bazel:worker_python)" if controller else "-",
        ] + gate_args,
        data = depset(
            [":" + name + "_binary", "//tools/ci:rust-test-budget.json"] + binaries +
            kwargs.get("compile_data", []) + kwargs.get("srcs", []) + runtime + runtime_data,
        ).to_list() + postgres,
        deps = ["//tools/ci:rust_gate"],
        tags = ["rust"] + tags,
        exec_properties = exec_properties,
        size = "large",
    )

def tl3_crate(package, crate, local_deps = [], features = [], build_script = False,
              bins = [], examples = {}, declared_tests = {}):
    """Build one workspace crate and every test target `cargo test --all-targets` runs.

    Integration tests that follow Cargo's auto-discovery (tests/NAME.rs or
    tests/NAME/main.rs) come from a glob, so adding one needs no generation.
    declared_tests holds the [[test]] entries that do not.
    """
    directory = native.package_name()
    native.exports_files(["Cargo.toml", "BUILD.bazel"])
    native.filegroup(
        name = "sources",
        srcs = native.glob(
            ["src/**/*", "tests/**/*", "tools/**/*", "migrations/**", "*.toml", "*.md"],
            allow_empty = True,
        ),
    )
    compile_data = _COMPILE_DATA.get(directory, [])
    script = []
    if build_script:
        cargo_build_script(
            name = "build_script",
            srcs = ["build.rs"],
            crate_name = crate,
            edition = "2024",
            data = ["Cargo.toml"] + _BUILD_SCRIPT_DATA.get(directory, []),
        )
        script = [":build_script"]
    deps = all_crate_deps(normal = True) + local_deps
    rust_library(
        name = crate,
        crate_name = crate,
        crate_root = "src/lib.rs",
        srcs = [":sources"],
        compile_data = compile_data,
        edition = "2024",
        crate_features = features,
        aliases = aliases(),
        deps = deps + script,
    )
    for name in bins:
        rust_binary(
            name = name,
            crate_name = name.replace("-", "_"),
            crate_root = "src/bin/%s.rs" % name,
            srcs = [":sources"],
            edition = "2024",
            crate_features = features,
            aliases = aliases(),
            deps = deps + [":" + crate],
        )
    test_deps = all_crate_deps(normal = True, normal_dev = True) + local_deps + [":" + crate] + script
    for name, source in examples.items():
        rust_binary(
            name = "example_" + name,
            crate_name = name.replace("-", "_"),
            crate_root = source,
            srcs = [source],
            compile_data = _EXAMPLE_DATA.get(source, []),
            edition = "2024",
            aliases = aliases(normal = True, normal_dev = True),
            deps = test_deps,
        )

    options = dict(
        compile_data = compile_data,
        crate_features = features,
        deps = test_deps,
    )
    _crate_test("lib", package, crate, "src/lib.rs", True, options)
    for name in bins:
        _crate_test("bin", package, name, "src/bin/%s.rs" % name, True, options)
    for name, source in examples.items():
        _crate_test("example", package, name, source, True, options)
    declared = [spec["path"] for spec in declared_tests.values()]
    for source in native.glob(["tests/*.rs", "tests/*/main.rs"], allow_empty = True):
        if source not in declared:
            name = source.removeprefix("tests/").removesuffix(".rs").removesuffix("/main")
            _crate_test("test", package, name, source, True, options)
    for name, spec in declared_tests.items():
        _crate_test("test", package, name, spec["path"], spec.get("harness", True), options)

    # Light checks also run standalone unittest suites that live in a crate.
    for source in native.glob(["tests/**/test_*.py"], exclude = ["**/fixtures/**"], allow_empty = True):
        tl3_py_test(
            name = source.removeprefix("tests/").removesuffix(".py").replace("/", "_"),
            srcs = [source],
        )

def _crate_test(kind, package, name, source, harness, options):
    binary = "%s/%s" % (package, name)
    for group in _GATE_GROUPS.get(binary, [""]):
        suffix = "-" + group if group else ""
        tl3_rust_test(
            name = "%s_test_%s%s" % (kind, name, suffix),
            binary_id = binary + suffix,
            crate_name = name.replace("-", "_"),
            crate_root = source,
            srcs = [":sources", source],
            compile_data = options["compile_data"] + _EXAMPLE_DATA.get(source, []),
            edition = "2024",
            crate_features = options["crate_features"],
            use_libtest_harness = harness,
            aliases = aliases(normal = True, normal_dev = True),
            deps = options["deps"],
            gate_args = ["--gate-group=" + group] if group else [],
        )
