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
            "kernel": ("kernel", 89),
            "snapshot": ("kernel::snapshot", 2),
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
            "retries": ("kernel::retries#run_failed", 1),
            "retry_next_attempt": ("kernel::retries#next_attempt", 1),
            "retry_replan_after_fix": ("kernel::retries#replan_after_fix", 1),
            "retry_stop": ("kernel::retries#stop_for_review", 1),
            "retry_failed_round": ("kernel::retries#failed_round", 1),
            "retry_next_rung": ("kernel::retries#next_rung", 1),
            "retry_out_of_room": ("kernel::retries#out_of_room", 1),
            "retry_plan": ("kernel::retries#plan_retry", 1),
            "retry_increment_rechecks": ("kernel::retries#increment_rechecks", 1),
            "retry_recheck_transition": ("kernel::retries#recheck_transition", 1),
            "retry_continue_recheck": ("kernel::retries#continue_recheck", 1),
            "retry_continue_fields": ("kernel::retries#continue_recheck_fields", 1),
            "retry_finish_recheck": ("kernel::retries#finish_recheck", 1),
            "retry_recheck": ("kernel::retries#recheck", 1),
            "acknowledgements": ("kernel::acknowledgements#on_ack", 1),
            "ack_confirmed": ("kernel::acknowledgements#confirmed", 1),
            "ack_confirmed_agenda": ("kernel::acknowledgements#confirmed_agenda", 1),
            "ack_delivery_failed": ("kernel::acknowledgements#delivery_failed", 1),
            "ack_failed_delivery_node": ("kernel::acknowledgements#failed_delivery_node", 1),
            "ack_effect_failed": ("kernel::acknowledgements#effect_failed", 1),
            "facts": ("kernel::facts#on_fact", 1),
            "facts_priority": ("kernel::facts#priority_fact", 1),
            "facts_pr_closed": ("kernel::facts#pr_closed_fact", 1),
            "facts_conflict": ("kernel::facts#conflict_fact", 1),
            "facts_declaration": ("kernel::facts#declaration_fact", 1),
            "facts_transition": ("kernel::facts#fact_transition", 1),
            "facts_incident_cleared": ("kernel::facts#incident_cleared", 1),
            "facts_pr_dequeued": ("kernel::facts#pr_dequeued", 1),
            "facts_outside_push": ("kernel::facts#outside_push", 1),
            "facts_upstream_stalled": ("kernel::facts#upstream_stalled", 1),
            "facts_ci_failed": ("kernel::facts#ci_failed", 1),
            "facts_ci_tests": ("kernel::facts#ci_tests", 1),
            "facts_ci_infrastructure_fact": ("kernel::facts#ci_infrastructure_fact", 1),
            "facts_cancel_enqueue": ("kernel::facts#cancel_enqueue", 1),
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
            "scheduler": ("kernel::scheduler#advance", 1),
            "scheduler_pool_free": ("kernel::scheduler#pool_free", 2),
            "scheduler_admits": ("kernel::scheduler#admits", 2),
            "scheduler_slot_free": ("kernel::scheduler#slot_free", 2),
            "scheduler_pool_order": ("kernel::scheduler#earlier_in_pool", 1),
            "scheduler_older_overlap": ("kernel::scheduler#older_overlap", 2),
            "scheduler_first_in_line": ("kernel::scheduler#first_in_line", 3),
            "steps": ("kernel::steps", 2),
            "settling": ("kernel::settling", 3),
            "progress_theorem": ("kernel::progress_theorem", 2),
            "witness": ("kernel::witness", 2),
            "progress_pool": ("kernel::progress_pool", 6),
            "progress_global": ("kernel::progress_global", 7),
            "gate_transition": ("kernel::gate_transition", 1),
            "ranks": ("kernel::ranks", 30),
            "footprint": ("kernel::footprint", 25),
            "ids": ("kernel::ids", 6),
            "controller_boundary": ("controller_boundary", 2),
            "command_headroom": ("root", 1),
            "retirement_retry": ("kernel::retirement_retry", 1),
            "retirement_retry_rules": ("kernel::retirement_retry_rules", 4),
            "retirement_retry_apply": ("kernel::retirement_retry_apply#verify_retry_apply", 1),
            "retirement_retry_world": ("kernel::retirement_retry_apply#fact_preserves_world", 1),
            "retirement_retry_result_rules": ("kernel::retirement_retry_apply#result_rules", 1),
            "retirement_retry_transition": ("kernel::retirement_retry_apply#fact_transition_rules", 1),
            "retirement_retry_frame": ("kernel::retirement_retry_frame", 1),
            "retirement_retry_witness": ("kernel::retirement_retry_witness", 1),
            "retirement_publishing": ("kernel::retirement_publishing", 3),
            "retirement_gate": ("kernel::retirement_gate", 5),
            "retirement_gate_witness": ("kernel::retirement_gate_witness", 2),
            "retirement_wide": ("kernel::retirement_wide", 3),
            "retirement": ("kernel::retirement", 2),
            "retirement_frames": ("kernel::retirement_frames", 3),
            "retirement_node": ("kernel::retirement_node", 1),
            "retirement_apply": ("kernel::retirement_apply", 2),
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
