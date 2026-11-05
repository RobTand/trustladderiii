"""Small required Verus proofs with declared source, spec and toolchain inputs."""

load("//tools/bazel:verus_toolchain.bzl", "VerusToolchainInfo")

def _verus_test_impl(ctx):
    if ctx.attr.expected_obligations <= 0:
        fail("verus_test requires positive expected_obligations")
    if not ctx.files.srcs:
        fail("verus_test must name the executable module under proof in srcs")
    toolchain = ctx.attr.toolchain[VerusToolchainInfo]
    launcher = ctx.actions.declare_file(ctx.label.name + ".sh")
    ctx.actions.write(launcher, """#!/bin/bash
set -euo pipefail
cd "${RUNFILES_DIR:-$0.runfiles}/%s"
exec "%s" "%s" "%s" "%s" "%s" "%s" "%s" "%s"
""" % (
        ctx.workspace_name,
        ctx.executable._runner.short_path,
        toolchain.verifier.short_path,
        toolchain.z3.short_path,
        toolchain.sysroot.short_path,
        ctx.file.src.short_path,
        ctx.attr.expected_obligations,
        ctx.label,
        ctx.attr.module,
    ), is_executable = True)
    runfiles = ctx.runfiles(files = [ctx.file.src, ctx.executable._runner] + ctx.files.srcs + ctx.files.specs,
                           transitive_files = toolchain.files)
    runfiles = runfiles.merge(ctx.attr._runner[DefaultInfo].default_runfiles)
    return [DefaultInfo(executable = launcher, runfiles = runfiles)]

verus_test = rule(
    implementation = _verus_test_impl,
    test = True,
    attrs = {
        "src": attr.label(allow_single_file = [".rs"], mandatory = True),
        "srcs": attr.label_list(allow_files = [".rs"], mandatory = True),
        "specs": attr.label_list(allow_files = [".rs"]),
        "expected_obligations": attr.int(mandatory = True),
        "module": attr.string(),
        "toolchain": attr.label(default = "@verus_linux_amd64//:toolchain", providers = [VerusToolchainInfo]),
        "_runner": attr.label(default = "//crates/mergify:verus-runner", executable = True, cfg = "exec"),
    },
)

def verus_proofs():
    """Proof inventory in existing crate packages; generated BUILD files stay Cargo-only."""
    if native.package_name() == "crates/mergify":
        verus_test(
            name = "verus_delivery",
            src = "proofs/delivery.rs",
            srcs = ["src/decide/delivery_step.rs"],
            specs = ["proofs/delivery_spec.rs"],
            expected_obligations = 2,
            tags = ["rust", "verus"],
            size = "small",
            timeout = "short",
            target_compatible_with = ["@platforms//cpu:x86_64"],
            exec_compatible_with = ["@platforms//os:linux", "@platforms//cpu:x86_64"],
            exec_properties = {"EstimatedCPU": "1", "EstimatedComputeUnits": "1", "EstimatedMemory": "1GB"},
        )

    if native.package_name() == "crates/controller":
        native.exports_files(["src/kernel_boundary.rs"])
        verus_test(
            name = "verus_budget_cutoff",
            src = "proofs/budget_cutoff.rs",
            srcs = ["src/budget/cutoff.rs"],
            expected_obligations = 1,
            tags = ["rust", "verus"],
            size = "small",
            timeout = "short",
            target_compatible_with = ["@platforms//cpu:x86_64"],
            exec_compatible_with = ["@platforms//os:linux", "@platforms//cpu:x86_64"],
            exec_properties = {"EstimatedCPU": "1", "EstimatedComputeUnits": "1", "EstimatedMemory": "1GB"},
        )
    if native.package_name() == "crates/controller-core":
        # Group F: H16/C4, production decisions and concrete branch witnesses.
        worker_event_groups = {
            "quota_model": ("worker_events#quota_model", 1),
            "reply_step": ("worker_events#reply_step", 1),
            "progress_due": ("worker_events#progress_due", 1),
            "progress_signal": ("worker_events#progress_signal", 1),
            "claim_decision": ("worker_events#claim_decision", 1),
            "refresh_gate": ("worker_events#refresh_gate", 1),
            "end_decision": ("worker_events#end_decision", 1),
            "witnesses": ("worker_events_witnesses", 4),
        }
        for name, (module, count) in worker_event_groups.items():
            verus_test(
                name = "verus_worker_events_" + name,
                src = "verus/worker_events.rs",
                srcs = ["src/worker_events.rs"],
                specs = ["verus/worker_events_witnesses.rs"],
                expected_obligations = count,
                module = module,
                tags = ["rust", "verus"],
                size = "small",
                timeout = "short",
                target_compatible_with = ["@platforms//cpu:x86_64"],
                exec_compatible_with = ["@platforms//os:linux", "@platforms//cpu:x86_64"],
                exec_properties = {"EstimatedCPU": "1", "EstimatedComputeUnits": "1", "EstimatedMemory": "1GB"},
            )

    if native.package_name() == "crates/kernel":
        # Every body called by step is checked in one of these groups. Module
        # filters bound solver context; exact counts refuse empty selections.
        # #806: the required proof_map test rejects missing, duplicate and stale entries.
        unverified_functions = {
            "kernel::pool_key": "Diagnostic pool-order tuple; real scheduling and its order predicates are verified.",
            "kernel::Phase::name": "Read-only phase label; ordinary Rust outside the scoped Verus core.",
            "kernel::Priority::is_unranked": "Read-only classification API; ordinary Rust outside the scoped Verus core.",
            "kernel::Outbox::get": "Read-only outbox inspection used by tests and adapters.",
            "kernel::Outbox::is_empty": "Read-only outbox inspection used by tests and adapters.",
            "kernel::Outbox::contains": "Read-only outbox inspection used by tests and adapters.",
            "kernel::World::new": "Container constructor; the verified constructors operate on typed nodes.",
            "kernel::World::measure": "Bounded diagnostic u64 sum; #805 proves the unbounded natural measure.",
            "kernel::World::valid": "Diagnostic consistency query; the production checker itself is verified.",
            "kernel::proof_of_event": "Exhaustive proof-map metadata; compiled and checked by proof_map.",
            "kernel::proofs_of_phase": "Exhaustive proof-map metadata; compiled and checked by proof_map.",
            "kernel::settle_world": "Reconciliation wrapper outside the scoped event step theorem.",
            "kernel::step_traced": "Diagnostic trace wrapper; the actual apply and settle bodies are verified.",
            "kernel::footprint::Footprint::default": "Default delegates to the verified inline constructor.",
            "kernel::footprint::World::intern_footprint": "Iterator collection at the boundary; sorted interning body is verified.",
            "kernel::footprint::World::files": "Read-only iterator API outside the scoped Verus core.",
            "kernel::footprint::World::file_sets": "Read-only storage inspection outside the scoped Verus core.",
            "kernel::dependencies::World::dependencies": "Read-only storage inspection outside the scoped Verus core.",
            "kernel::tests::scan_budget_checks_the_actual_u32_width": "Rust unit test; absent from the Verus cfg.",
            "kernel::tests::settle_budget_reserves_all_six_internal_moves_per_open_node": "Rust unit test; absent from the Verus cfg.",
            "kernel::footprint::Footprint::any": "Frozen Kani-only constructor; absent from the Verus cfg.",
            "kernel::queries::Node::waiting_on": "Read-only presentation API outside the scoped kernel proof.",
            "kernel::queries::Node::operation_pending": "Read-only presentation API outside the scoped kernel proof.",
            "kernel::queries::Node::effect_cancelling": "Read-only presentation API outside the scoped kernel proof.",
        }
        groups = {
            "kernel": ("kernel", 91),
            "events": ("kernel::events", 1),
            "allocation": ("kernel::allocation#ask_rob", 1),
            "allocation_set_phase": ("kernel::allocation#set_phase", 1),
            "allocation_abandon_operation": ("kernel::allocation#abandon_operation", 1),
            "allocation_end_operation": ("kernel::allocation#end_operation", 1),
            "allocation_enter_stopping": ("kernel::allocation#enter_stopping", 1),
            "allocation_merge_credit": ("kernel::allocation#merge_credit", 1),
            "allocation_close_node": ("kernel::allocation#close_node", 1),
            "allocation_raise_incident": ("kernel::allocation#raise_incident", 1),
            "allocation_emit": ("kernel::allocation#emit", 1),
            "allocation_emit_node": ("kernel::allocation#emit_node", 1),
            "reply_admission": ("kernel::reply_admission", 4),
            "ingest": ("kernel::ingest", 3),
            "work_replies": ("kernel::work_replies", 4),
            "retries": ("kernel::retries", 7),
            "acknowledgements": ("kernel::acknowledgements", 6),
            "facts": ("kernel::facts#on_fact", 1),
            "facts_transition": ("kernel::facts#fact_transition", 1),
            "facts_incident_cleared": ("kernel::facts#incident_cleared", 1),
            "facts_pr_dequeued": ("kernel::facts#pr_dequeued", 1),
            "facts_ci_failed": ("kernel::facts#ci_failed", 1),
            "facts_infrastructure_retry": ("kernel::facts#infrastructure_retry", 1),
            "facts_ci_infrastructure": ("kernel::facts#ci_infrastructure", 1),
            "facts_merge": ("kernel::facts#pr_merged", 1),
            "facts_merge_open": ("kernel::facts#merge_open", 1),
            "commands": ("kernel::commands", 1),
            "answers": ("kernel::answers", 1),
            "command_helpers": ("kernel::command_helpers", 2),
            "continuations": ("kernel::continuations", 1),
            "checking": ("kernel::checking", 15),
            "dependencies": ("kernel::dependencies", 35),
            "progress": ("kernel::progress", 10),
            "scheduler": ("kernel::scheduler", 13),
            "steps": ("kernel::steps", 2),
            "settling": ("kernel::settling", 3),
            "progress_theorem": ("kernel::progress_theorem", 2),
            "witness": ("kernel::witness", 2),
            "progress_pool": ("kernel::progress_pool", 6),
            "progress_global": ("kernel::progress_global", 7),
            "ranks": ("kernel::ranks", 30),
            "footprint": ("kernel::footprint", 22),
            "ids": ("kernel::ids", 6),
            "controller_boundary": ("controller_boundary", 2),
            "command_headroom": ("root", 1),
            "retirement": ("kernel::retirement", 7),
            "retirement_witness": ("kernel::retirement_witness", 2),
            "retirement_merge_witness": ("kernel::retirement_merge_witness", 2),
            "retirement_merge_waiting_witness": ("kernel::retirement_merge_waiting_witness", 2),
            "retirement_terminal_fact_witness": ("kernel::retirement_terminal_fact_witness", 2),
        }
        for name, (module, count) in groups.items():
            verus_test(
                name = "verus_" + name,
                src = "verus/progress.rs",
                srcs = native.glob(["src/**/*.rs"], exclude = ["src/proofs.rs"]) + ["//crates/controller:src/kernel_boundary.rs"],
                expected_obligations = count,
                module = module,
                specs = native.glob(["verus/*.rs"]),
                tags = ["rust", "verus"],
                size = "small",
                timeout = "short",
                target_compatible_with = ["@platforms//cpu:x86_64"],
                exec_compatible_with = ["@platforms//os:linux", "@platforms//cpu:x86_64"],
                exec_properties = {"EstimatedCPU": "1", "EstimatedComputeUnits": "1", "EstimatedMemory": "1GB"},
            )
