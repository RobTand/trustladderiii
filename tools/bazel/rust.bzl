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
load("//tools/bazel:test_sizing.bzl", "test_size")
load("//tools/bazel:verus.bzl", "verus_proofs")

# Cargo tests read env!("CARGO_MANIFEST_DIR") and some require it absolute.
# rust_runner.py points this fixed path at the test's runfiles. Each test needs
# a private /tmp, which remote containers and Bazel's sandbox both provide.
WORKSPACE = "/tmp/tl3-workspace"

_POSTGRES = ["crates/controller", "crates/store"]

# Files that include_str! and build.rs read from outside the crate.
_COMPILE_DATA = {
    "crates/contracts": ["//workers:schemas", "//workers/tests:footprint_paths"],
    "crates/controller": ["//workers:schemas"],
    "crates/config": ["//deploy/dl380g10:models.toml"],
}
_BUILD_SCRIPT_DATA = {
    "crates/controller": ["//workers:schemas", "//workers/src:prompts"],
}
_EXAMPLE_DATA = {
    "examples/checker-vectors.rs": ["//crates/controller:tests/worker_checker/oracle.rs"],
    "//deploy/dl380g10:helper.rs": ["//deploy/github-app:manifest.json"],
}

# Files other crates' tests read at run time.
_RUNTIME = {"crates/harness-core": [
    "//harness:core_vectors",
    "//harness:test-vectors/summary/python-envelope.json",
    "//harness:test-vectors/summary/python-numbers.json",
]}

# rust_gate.py runs the controller's github binary as two timed groups.
_GATE_GROUPS = {"tl3-controller/github": ["flow", "notices"]}

# These custom harnesses select complete scenario groups through scenario_shard.rs.
_SCENARIO_SHARDS = {
    "tl3-controller/github-flow": 50,
    "tl3-controller/github-notices": 32,
    "tl3-controller/publishing": 38,
    "tl3-controller/simulation": 16,
    "tl3-controller/workers": 32,
}

# rust_runner.py splits these libtest binaries by the measured test times in
# libtest-shards.json (#731). The controller library ran 362 tests serially in
# one 249-324 s action.
_LIBTEST_SHARDS = {
    # #830: 62.427 s serial, measured at 34901a132a (da4bea96).
    "tl3-controller/direct_merge": 8,
    "tl3-controller/quota_hold": 16,
    "tl3-controller/input_resources": 10,
    "tl3-controller/stderr_incidents": 10,
    "tl3-controller/approval_race": 27,
    "tl3-controller/budget_parity": 3,
    "tl3-controller/p5_checker": 14,
    "tl3-controller/operations_quota": 4,
    "tl3-controller/worker_budget_orchestration": 21,
    "tl3-controller/tl3_controller": 32,
    "tl3-controller/tl3_controller_loom": 4,
    "tl3-controller/redaction_generated": 8,
}

# Inputs read by controller tests, rather than by the controller library.
_CONTROLLER_TEST_DATA = [
    "//crates/config:sources",
    "//crates/github:sources",
    "//crates/store:sources",
]
_CONTROLLER_TEST_COMPILE_DATA = {
    "tests/writer_blocker.rs": ["//workers/tests:fixtures"],
}
# Only these scenarios execute tl3_workers. Its imports include tl3_harness.
_CONTROLLER_WORKERS = [
    "test_test_stderr_incidents",
    "test_test_harness_summary",
    "test_test_redaction_adapter",
    "test_test_redaction_generated",
    "test_test_redaction_parity",
    "lib_test_tl3_controller",
    "test_test_github-flow",
    "test_test_publishing",
    "test_test_workers",
    "test_test_writer_references",
    "test_test_budget_parity",
    "test_test_worker_checker",
    "test_test_worker_budget_orchestration",
    "test_test_p5_checker",
]
_CONTROLLER_BINARIES = {
    "test_test_reply_violations": [":tl3"],
    "test_test_run_backend_cli": [":tl3-run-backend"],
    "test_test_worker_supervisor": [":tl3-worker-supervisor"],
    "test_test_workers": [":tl3"],
    "test_test_console_commands": [":tl3d"],
    "test_test_redaction_policy": [":tl3"],
    "test_test_github-flow": [":tl3d"],
    # Both groups compile github.rs, including its env!("CARGO_BIN_EXE_tl3d").
    "test_test_github-notices": [":tl3d"],
    "test_test_simulation": [":tl3", ":tl3d"],
    "test_test_replay_store": [":tl3"],
    # FX-REQUAL-43 OQ-1: every deployed binary, plus the canary example as positive control.
    "test_test_activation_qualification_absent": [
        ":tl3",
        ":tl3d",
        ":tl3-run-backend",
        ":tl3-worker-supervisor",
        ":example_tl3-deploy",
        ":example_fx-stage-packet",
    ],
}
# These tests stage the trusted harness into a sealed check action, even when
# the queue fixture supplies the result instead of executing that action.
_CONTROLLER_RUNTIME = {
    "test_test_p5_checker": ["//workers/tests:budget_orchestration", "//workers/tests:p5_recordings", "//workers/tests:p5_references", ":example_p5-vectors"],
    "test_test_harness_summary": ["//workers/tests:fixtures", "//workers/tests:harness_io_schedule"],
    "test_test_worker_budget_orchestration": ["//workers/tests:budget_orchestration"],
    "test_test_worker_checker": ["//workers/tests:checker_recordings", ":example_checker-vectors"],
    "test_test_redaction_policy": ["//workers/src:redaction_contract"],
    "test_test_redaction_adapter": ["//workers/tests:redaction_branches"],
    "test_test_redaction_generated": ["//workers/tests:redaction_generated"],
    "test_test_redaction_parity": ["//workers/tests:redaction_branches"],
    "lib_test_tl3_controller": [
        "//workers/tests:p5_references",
        "//harness:project_files",
        "//workers/tests:worker_process",
        "//workers/tools:scripts",
        "//workers/src:prompts",
        "//workers/tests:fixtures",
        "//workers/tests:prompt_render_recordings",
    ],
    # The shared harness check-report vectors (#734) the typed contracts parse (#741).
    "test_test_contracts": ["//harness:project_files"],
    "test_test_hindsight": ["//workers/src:role_files"],
    "test_test_publishing": ["//harness/src/tl3_harness"],
    "test_test_sandbox_gate": ["//deploy/sandbox:evidence"],
    "test_test_activation_qualification_absent": [
        ":BUILD.bazel",
        ":Cargo.toml",
        ":library_sources",
        "//:Cargo.toml",
        "//deploy/dl380g10:build-release.sh",
        "//deploy/dl380g10:verify.sh",
    ] + ["//crates/%s:Cargo.toml" % crate for crate in [
        "config", "contracts", "controller-core", "delivery", "github", "harness-core",
        "kernel", "plan-check", "store", "verus-runner", "worktrees",
    ]],
    "test_test_workers": [
        "//harness/src/tl3_harness",
        "//workers/tests:worker_process",
        "//workers/tools:scripts",
    ],
}

# Shared modules and runtime fixtures outside tests/NAME/. The per-test glob
# includes its own module tree; these are the cross-tree reads Cargo cannot see.
_CONTROLLER_TEST_SOURCES = {
    "tests/run_backend_cli.rs": ["tests/python/fx_native_scratch_helper.py", "tests/python/fx_scratch_helper.py"],
    "tests/direct_merge.rs": ["tests/support/**"],
    "tests/input_resources.rs": ["tests/support/**"],
    "src/lib.rs": ["tests/worker_events_io/**", "tests/support/input_resource_world.rs", "tests/fail_fast/pb-2781.json", "tests/fixtures/dispatch_recovery_original.rs", "tests/fixtures/dispatch-recovery.proptest-regressions", "tests/fixtures/dispatch-conformance.proptest-regressions", "tests/fixtures/dispatch-conformance-regressions.json", "tests/fixtures/dispatch-restart-regressions.json"],
    "tests/worker_checker.rs": ["tests/worker_checker/**"],
    "tests/approval_race.rs": ["tests/support/github.rs", "tests/support/controller_restart.rs"],
    "tests/budget_parity/main.rs": ["tests/budget_parity/*.py", "tests/budget_parity/*.txt"],
    "tests/stderr_incidents.rs": ["tests/redaction_support/**"],
    "tests/redaction_parity.rs": ["tests/redaction_support/**"],
    "tests/redaction_generated.rs": ["tests/redaction_support/**"],
    "tests/worker_supervisor/main.rs": ["tests/worker_supervisor/legacy.py", "tests/worker_supervisor/restart.rs", "tests/support/ownership.rs"],
    "tests/console_commands.rs": ["tests/fixtures/decisions/**", "tests/support/scenario.rs", "tests/support/command.rs"],
    "tests/hindsight.rs": ["README.md"],
    "tests/fail_fast.rs": ["tests/support/**"],
    "tests/fixture_contracts.rs": ["tests/support/**"],
    "tests/core_defects.rs": ["tests/worktree_fixture/**", "tests/support/ownership.rs"],
    "tests/crash_safety.rs": ["tests/crash/**"],
    "tests/github.rs": ["tests/support/**", "tests/fixtures/decisions/**", "tests/fixtures/steering/**", "tests/worker_model.py", "tests/worker_replies.py", "tests/worker_transport.py"],
    "tests/looks.rs": ["tests/worktree_fixture/**", "tests/support/ownership.rs"],
    "tests/publishing_drain.rs": ["tests/publishing/drain_deadline.rs"],
    "tests/publishing.rs": ["tests/support/**", "tests/worktree_fixture/**"],
    "tests/reproducing_rebase.rs": ["tests/worktree_fixture/**", "tests/support/ownership.rs"],
    "tests/scenario_watchdog.rs": ["tests/support/scenario.rs"],
    "tests/scope.rs": ["tests/support/**", "tests/worktree_fixture/**"],
    "tests/simulation.rs": ["tests/worktree_fixture/**", "tests/support/scenario_shard.rs", "tests/support/ownership.rs", "tests/support/command.rs"],
    "tests/upgrade_replay.rs": ["tests/support/ownership.rs"],
    "tests/workers.rs": [
        "tests/support/**",
        "tests/fixtures/narrowing/**",
        "tests/fixtures/decisions/**",
        "tests/maker/**",
        "tests/worker_model.py",
        "tests/worker_replies.py",
        "tests/worker_transport.py",
    ],
    "tests/worktrees.rs": ["tests/worktree_fixture/**", "tests/support/ownership.rs"],
    "tests/writer_references.rs": ["tests/writer_reference_model.py"],
}

# CC-E: reuse existing topic targets for delivery coverage without fanout growth.
_CONTROLLER_TEST_SOURCES["tests/approval_race.rs"] += ["tests/delivery_conformance/**", "tests/delivery_fence/**"]
_CONTROLLER_TEST_SOURCES["tests/publication_io.rs"] = _CONTROLLER_TEST_SOURCES.get("tests/publication_io.rs", []) + ["tests/delivery_schedule/**"]
_CONTROLLER_TEST_SOURCES["tests/replay_check.rs"] = _CONTROLLER_TEST_SOURCES.get("tests/replay_check.rs", []) + ["tests/delivery_machine/**"]
# One approval schedule per shard, including the eight controller and two fence cases.
_LIBTEST_SHARDS["tl3-controller/approval_race"] = 37

def tl3_rust_test(name, binary_id, gate_args, **kwargs):
    package = native.package_name()
    controller = package == "crates/controller"
    binaries = _CONTROLLER_BINARIES.get(name, []) if controller else []
    # Every controller test can reach the adapter, including through shared fixtures.
    if controller and ":tl3" not in binaries:
        binaries = binaries + [":tl3"]
    if controller and ":tl3-worker-supervisor" not in binaries:
        binaries = binaries + [":tl3-worker-supervisor"]
    workers = controller and name in _CONTROLLER_WORKERS
    env = {"CARGO_MANIFEST_DIR": WORKSPACE + "/" + package}
    for binary in binaries:
        env["CARGO_BIN_EXE_" + binary[1:]] = WORKSPACE + "/$(rootpath " + binary + ")"
    runtime = _CONTROLLER_RUNTIME.get(name, []) if controller else _RUNTIME.get(package, [])
    if package == "crates/config" and name.startswith("test_test_tokenizer_"):
        runtime = runtime + ["@o200k_base//file"]
    if workers:
        runtime = runtime + ["//tools/bazel:worker_python", "@o200k_base//file"]
    postgres = select({
        "@platforms//cpu:aarch64": ["@postgres_linux_arm64//:all"],
        "//conditions:default": ["@postgres_linux_amd64//:all"],
    }) if package in _POSTGRES else []
    tags, exec_properties = nonhermetic(name)
    exec_properties.update(test_size(package, name, postgres = package in _POSTGRES))
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
        srcs = ["//tools/bazel:rust_runner.py", "//tools/bazel:runtime.py", "//tools/bazel:libtest_shards.py"],
        main = "//tools/bazel:rust_runner.py",
        args = [
            "$(rootpath :%s_binary)" % name,
            binary_id,
            harness,
            "$(rootpath //tools/bazel:worker_python)" if workers else "-",
        ] + gate_args,
        data = depset(
            [":" + name + "_binary", "//tools/ci:rust-test-budget.json", "//tools/bazel:libtest-shards.json"] +
            (["//tools/ci:controller_test_runner.sh"] if controller else []) + binaries +
            kwargs.get("compile_data", []) + kwargs.get("srcs", []) + runtime + runtime_data,
        ).to_list() + postgres,
        deps = ["//tools/ci:rust_gate"],
        env = {"TL3_BINARY": WORKSPACE + "/$(rootpath :tl3)"} if controller else {},
        tags = ["rust"] + tags,
        exec_properties = exec_properties,
        size = "large",
        shard_count = _SCENARIO_SHARDS.get(binary_id, _LIBTEST_SHARDS.get(binary_id, 0)),
        # Keep #472 while worker shards need CPU calibration and wall_only entries remain.
        timeout = "eternal" if binary_id == "tl3-controller/workers" else "long",
    )

def _feature_variant(crate, required):
    return crate + "_" + "_".join(sorted(required)).replace("-", "_")

def _example_options(name, crate, example_features, options):
    """An example with Cargo required-features links the library variant built with them."""
    required = sorted(example_features.get(name, []))
    if not required:
        return options
    variant = ":" + _feature_variant(crate, required)
    return dict(
        compile_data = options["compile_data"],
        crate_features = options["crate_features"] + required,
        deps = [variant if dep == ":" + crate else dep for dep in options["deps"]],
    )

def tl3_crate(package, crate, local_deps = [], features = [], build_script = False,
              bins = [], examples = {}, example_features = {}, declared_tests = {}):
    """Build one workspace crate and every test target `cargo test --all-targets` runs.

    Integration tests that follow Cargo's auto-discovery (tests/NAME.rs or
    tests/NAME/main.rs) come from a glob, so adding one needs no generation.
    declared_tests holds the [[test]] entries that do not.

    example_features holds Cargo `required-features` of examples. Each such
    example links its own library variant built with those features; the
    library, binaries and every other target keep `features` only.
    """
    directory = native.package_name()
    verus_proofs()
    native.exports_files(["Cargo.toml", "BUILD.bazel"])
    native.filegroup(
        name = "sources",
        srcs = native.glob(
            ["src/**/*", "tests/**/*", "tools/**/*", "migrations/**", "*.toml", "*.md"],
            allow_empty = True,
        ),
    )
    separate_library = directory in ["crates/controller", "crates/kernel", "crates/store", "crates/contracts"]
    if separate_library:
        native.filegroup(
            name = "library_sources",
            # Proof drivers and crate tooling are not linked into a Rust library.
            srcs = native.glob(
                ["src/**/*", "migrations/**"],
                exclude = ["src/proofs.rs"] + (["src/bin/**"] if directory == "crates/contracts" else []),
                allow_empty = True,
            ),
        )
    library_sources = [":library_sources"] if separate_library else [":sources"]
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
        srcs = library_sources,
        compile_data = compile_data,
        edition = "2024",
        crate_features = features,
        aliases = aliases(),
        deps = deps + script,
    )
    variants = {}
    for required in example_features.values():
        variant = _feature_variant(crate, required)
        if variant in variants:
            continue
        variants[variant] = True
        rust_library(
            name = variant,
            crate_name = crate,
            crate_root = "src/lib.rs",
            srcs = library_sources,
            compile_data = compile_data,
            edition = "2024",
            crate_features = features + sorted(required),
            aliases = aliases(),
            deps = deps + script,
            # Example-only feature variant; deployed binaries never link it.
            visibility = ["//visibility:private"],
        )
    for name in bins:
        standalone_verifier = name == "verus-runner" and directory == "crates/verus-runner"
        binary_sources = ["src/bin/%s.rs" % name] if standalone_verifier else library_sources
        if directory == "crates/contracts":
            binary_sources = library_sources + ["src/bin/%s.rs" % name]
        rust_binary(
            name = name,
            crate_name = name.replace("-", "_"),
            crate_root = "src/bin/%s.rs" % name,
            srcs = binary_sources,
            edition = "2024",
            crate_features = features,
            aliases = aliases(),
            deps = ["@crates//:serde_json"] if standalone_verifier else deps + [":" + crate],
        )
    test_deps = all_crate_deps(normal = True, normal_dev = True) + local_deps + [":" + crate] + script
    options = dict(
        compile_data = compile_data,
        crate_features = features,
        deps = test_deps,
    )

    for name, source in examples.items():
        qualification_sources = native.glob(["examples/fx_qualification/**/*.rs", "tools/fx_qualification/**/*.rs", "tools/generate_fx_qualification.rs"]) if name == "fx-stage-packet" else (native.glob(["tools/fx_qualification/**/*.rs"]) if name == "generate-fx-qualification" else [])
        rust_binary(
            name = "example_" + name,
            crate_name = name.replace("-", "_"),
            crate_root = source,
            srcs = [source] + qualification_sources,
            compile_data = _EXAMPLE_DATA.get(source, []),
            edition = "2024",
            crate_features = _example_options(name, crate, example_features, options)["crate_features"],
            aliases = aliases(normal = True, normal_dev = True),
            deps = _example_options(name, crate, example_features, options)["deps"],
        )

    _crate_test("lib", package, crate, "src/lib.rs", True, options)
    if directory == "crates/controller":
        loom_options = dict(options)
        loom_options["crate_features"] = features + ["loom"]
        loom_options["rustc_flags"] = ["--cfg=loom"]
        loom_options["deps"] = [dep for dep in test_deps if dep != ":" + crate]
        _crate_test("lib", package, crate + "_loom", "src/lib.rs", True, loom_options)
    for name in bins:
        binary_options = dict(options)
        if name == "verus-runner" and directory == "crates/verus-runner":
            binary_options["deps"] = ["@crates//:serde_json"]
        _crate_test("bin", package, name, "src/bin/%s.rs" % name, True, binary_options)
    for name, source in examples.items():
        _crate_test("example", package, name, source, True, _example_options(name, crate, example_features, options))
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
    sources = [":sources", source]
    if native.package_name() in ["crates/kernel", "crates/store"]:
        sources = [":library_sources", source] + native.glob([source.removesuffix(".rs") + "/**"], allow_empty = True)
        if native.package_name() == "crates/kernel" and (kind == "lib" or source == "tests/proof_map.rs"):
            sources = sources + native.glob(["tests/mark_handled_equivalence/*.rs"])
        if source == "tests/proof_map.rs":
            sources = sources + ["src/proofs.rs"] + native.glob(["verus/*.rs"], exclude = ["verus/progress.rs"])
        if native.package_name() == "crates/kernel" and source == "tests/obligation_map.rs":
            sources = sources + ["tests/transition_covers.rs", "tests/fail_fast.rs", "tests/scenarios.rs", "tests/step_report.rs", "tests/dependency_safety.rs"]
        if native.package_name() == "crates/kernel" and source == "tests/random_walks.rs":
            sources = sources + ["tests/measure_equivalence/reference.rs"]
        if native.package_name() == "crates/store":
            sources = sources + native.glob(["tests/support/**/*.rs"], allow_empty = True)
    if kind == "bin" and name == "verus-runner" and native.package_name() == "crates/verus-runner":
        sources = [source]
    compile_data = options["compile_data"]
    if source in ["tests/proof_map.rs", "tests/obligation_map.rs"] and native.package_name() == "crates/kernel":
        compile_data = compile_data + ["//tools/bazel:verus.bzl"]
    if native.package_name() == "crates/store":
        compile_data = compile_data + native.glob(["tests/fixtures/**"], allow_empty = True)
    if native.package_name() == "crates/controller":
        sources = [":library_sources", source] + native.glob(
            (["tests/loom/**", "tests/fixtures/decisions/**", "tests/fixtures/narrowing/**", "tests/fixtures/worker-replies-d8e4f2a8.jsonl"] if kind == "lib" else
             ([source.removesuffix(".rs") + "/**"] if not source.startswith("//") else [])) +
            _CONTROLLER_TEST_SOURCES.get(source, []),
            allow_empty = True,
        )
        if name == "tl3_controller_loom":
            sources = sources + native.glob(["tests/support/ownership.rs"])
        if kind == "example" and name == "fx-stage-packet":
            sources = sources + native.glob(["examples/fx_qualification/**/*.rs", "tools/fx_qualification/**/*.rs", "tools/generate_fx_qualification.rs"])
        native.filegroup(name = kind + "_sources_" + name, srcs = sources)
        sources = [":" + kind + "_sources_" + name]
        compile_data = compile_data + _CONTROLLER_TEST_DATA + _CONTROLLER_TEST_COMPILE_DATA.get(source, [])
        if kind == "lib":
            compile_data = compile_data + _CONTROLLER_RUNTIME.get("lib_test_tl3_controller" if name == "tl3_controller_loom" else "lib_test_" + name, [])
        if name in ["tl3_controller", "tl3_controller_loom", "generate-fx-qualification", "fx-stage-packet"]:
            compile_data = compile_data + native.glob(["qualification/**/*.json", "tools/generate_fx_qualification.rs", "tools/fx_qualification/**/*.rs"])
    for group in _GATE_GROUPS.get(binary, [""]):
        suffix = "-" + group if group else ""
        tl3_rust_test(
            name = "%s_test_%s%s" % (kind, name, suffix),
            binary_id = binary + suffix,
            crate_name = name.replace("-", "_"),
            crate_root = source,
            srcs = sources,
            compile_data = compile_data + [
                label
                for label in _EXAMPLE_DATA.get(source, [])
                if label not in options["compile_data"]
            ],
            edition = "2024",
            crate_features = options["crate_features"],
            rustc_flags = options.get("rustc_flags", []),
            use_libtest_harness = harness,
            aliases = aliases(normal = True, normal_dev = True),
            deps = options["deps"],
            # Shared-mount Flux fixtures run in the admitted PB check script. RBE
            # cannot create the fixed /mnt/shared output root; select its wire tests.
            # The two retained launch-failure lib fixtures likewise execute in PB.
            # The release-boundary ports run on Flux.
            gate_args = (["--libtest-select=cli_schema", "--libtest-select=malformed_requests", "--libtest-select=optional_reviewed", "--libtest-select=protocol_bounds"] if name == "run_backend_cli" else
                         (["--libtest-select=loom_"] if name == "tl3_controller_loom" else
                          (["--libtest-select=staging_namespace_survives_consumer_argument_validation", "--libtest-select=qualification_gate_refuses_records_and_production_still_requires_one", "--libtest-select=fx_qualification::"] if kind == "example" and name == "fx-stage-packet" else
                           (["--skip=adapters::run_backend::flux::launch_failure::tests::retained_fbelmqkrh5_success_keeps_its_checks_verdict_and_evidence",
                            "--skip=adapters::run_backend::flux::launch_failure::tests::retained_fbpzb5i8yp_without_launcher_exit_cannot_be_upgraded",
                            "--skip=adapters::flux_checks::activation::tests::qualified_identity_and_exact_planned_deployment_pass_staging_boundary",
                            "--skip=adapters::flux_checks::activation::tests::absent_wrong_hash_unknown_qualification_and_generation_refuse_with_fields_only",
                            "--skip=adapters::flux_checks::activation::tests::planned_scope_requires_explicit_success_and_cleanup_and_never_completed_claims",
                            "--skip=adapters::flux_checks::activation::tests::bundle_set_hash_profile_preflight_options_and_cli_substitution_refuse",
                            "--skip=adapters::flux_checks::activation::tests::sealed_but_unqualified_full_closure_and_fault_interpreter_refuse",
                            "--skip=adapters::flux_checks::activation::tests::missing_changed_cleanup_and_extra_unlisted_module_refuse",
                            "--skip=adapters::flux_checks::activation::tests::tags_runtime_substitution_and_environment_override_cannot_grant_activation",
                            "--skip=adapters::flux_checks::activation::tests::environment_override_child",
                            "--skip=adapters::flux_checks::activation::tests::qualified_binding_passes_real_controller_capacity_and_startup_activation",
                            "--skip=adapters::flux_checks::activation::tests::release_boundary::a_running_controller_stages_its_qualified_release_after_producer_removal",
                            "--skip=adapters::flux_checks::activation::tests::release_boundary::a_bundle_missing_at_startup_never_creates_an_available_stager",
                            "--skip=adapters::flux_checks::activation::tests::release_boundary::a_failed_input_copy_never_falls_back_to_the_producer",
                            "--skip=adapters::flux_checks::activation::tests::release_boundary::a_second_controller_load_never_touches_the_owners_input_or_bundle",
                            "--skip=adapters::flux_checks::activation::tests::release_boundary::a_current_switch_never_changes_the_qualified_module_set",
                            "--skip=adapters::flux_checks::activation::tests::release_boundary::a_truncated_bundle_is_never_staged",
                            "--skip=adapters::flux_checks::activation::tests::release_boundary::repeated_failed_preparations_never_publish_partial_inputs",
                            "--skip=adapters::flux_checks::activation::tests::release_boundary::interrupted_preparations_preserve_owned_partial_and_completed_inputs",
                            "--skip=adapters::workers::execution::inputs::tests::direct_prepare_requires_deployment_activation_and_checks_actual_options"] if kind == "lib" and name == "tl3_controller" else
                            (["--gate-group=" + group] if group else []))))),
        )
