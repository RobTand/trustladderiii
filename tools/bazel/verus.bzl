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

        # Group B1: production recovery residue and full-domain witnesses.
        for name, count in [("budget", 10), ("eligibility", 10), ("schedule", 8), ("command", 6)]:
            verus_test(
                name = "verus_recovery_" + name,
                src = "proofs/recovery.rs",
                srcs = ["src/decide/recovery.rs"],
                expected_obligations = count,
                module = "recovery::" + name,
                tags = ["rust", "verus"],
                size = "small",
                timeout = "short",
                target_compatible_with = ["@platforms//cpu:x86_64"],
                exec_compatible_with = ["@platforms//os:linux", "@platforms//cpu:x86_64"],
                exec_properties = {"EstimatedCPU": "1", "EstimatedComputeUnits": "1", "EstimatedMemory": "1GB"},
            )
        # ctrl-core C1: production required-check/rerun residue and witnesses.
        verus_test(
            name = "verus_required",
            src = "proofs/required.rs",
            srcs = ["src/decide/required.rs"],
            specs = ["proofs/required_spec.rs"],
            expected_obligations = 33,
            tags = ["rust", "verus"],
            size = "small",
            timeout = "short",
            target_compatible_with = ["@platforms//cpu:x86_64"],
            exec_compatible_with = ["@platforms//os:linux", "@platforms//cpu:x86_64"],
            exec_properties = {"EstimatedCPU": "1", "EstimatedComputeUnits": "1", "EstimatedMemory": "1GB"},
        )

        # ctrl-core A1: shell residue, with no contract changes to enqueue::decide.
        verus_test(
            name = "verus_completion",
            src = "proofs/completion.rs",
            srcs = ["src/decide/completion.rs"],
            specs = ["proofs/completion_spec.rs"],
            expected_obligations = 44,
            tags = ["rust", "verus"],
            size = "small",
            timeout = "short",
            target_compatible_with = ["@platforms//cpu:x86_64"],
            exec_compatible_with = ["@platforms//os:linux", "@platforms//cpu:x86_64"],
            exec_properties = {"EstimatedCPU": "1", "EstimatedComputeUnits": "1", "EstimatedMemory": "1GB"},
        )
        # #743 M1a: real observe/check/facts decisions and original-domain witnesses.
        for name, module, count in [
            ("observe", "observe", 11),
            ("check", "check", 12),
            ("check_facts", "facts", 26),
            ("observe_witness", "observe_witness", 8),
            ("observe_properties", "observe_properties", 2),
        ]:
            verus_test(
                name = "verus_" + name,
                src = "proofs/observe.rs",
                srcs = [
                    "src/decide/check.rs",
                    "src/decide/observe.rs",
                    "src/decide/verification.rs",
                    "src/decide/facts.rs",
                ],
                specs = ["src/decide/observe_witness.rs", "proofs/observe_properties.rs"],
                module = module,
                expected_obligations = count,
                tags = ["rust", "verus"],
                size = "small",
                timeout = "short",
                target_compatible_with = ["@platforms//cpu:x86_64"],
                exec_compatible_with = ["@platforms//os:linux", "@platforms//cpu:x86_64"],
                exec_properties = {"EstimatedCPU": "1", "EstimatedComputeUnits": "1", "EstimatedMemory": "1GB"},
            )

        # ctrl-core A2: executable execute plans and body-heal decisions.
        verus_test(
            name = "verus_plan",
            src = "proofs/plan.rs",
            srcs = ["src/decide/plan.rs"],
            specs = ["proofs/plan_spec.rs"],
            expected_obligations = 22,
            tags = ["rust", "verus"],
            size = "small",
            timeout = "short",
            target_compatible_with = ["@platforms//cpu:x86_64"],
            exec_compatible_with = ["@platforms//os:linux", "@platforms//cpu:x86_64"],
            exec_properties = {"EstimatedCPU": "1", "EstimatedComputeUnits": "1", "EstimatedMemory": "1GB"},
        )

        verus_test(
            name = "verus_heal",
            src = "proofs/heal.rs",
            srcs = ["src/decide/heal.rs"],
            specs = ["proofs/heal_spec.rs"],
            expected_obligations = 23,
            tags = ["rust", "verus"],
            size = "small",
            timeout = "short",
            target_compatible_with = ["@platforms//cpu:x86_64"],
            exec_compatible_with = ["@platforms//os:linux", "@platforms//cpu:x86_64"],
            exec_properties = {"EstimatedCPU": "1", "EstimatedComputeUnits": "1", "EstimatedMemory": "1GB"},
        )
        # #743 M1b: real translation, fact decisions, and all original covers.
        for name, module, count in [
            ("translate", "translate", 11),
            ("decide_properties", "decide_properties", 9),
            ("decide_witness", "decide_witness", 8),
        ]:
            verus_test(
                name = "verus_" + name,
                src = "proofs/decide.rs",
                srcs = ["src/decide/check.rs", "src/decide/observe.rs", "src/decide/facts.rs", "src/decide/translate.rs", "src/decide/verification.rs"],
                specs = ["src/decide/decide_witness.rs", "proofs/decide_properties.rs"],
                module = module,
                expected_obligations = count,
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
        verus_test(
            name = "verus_budget_limits",
            src = "proofs/budget_limits.rs",
            srcs = ["src/budget/limits.rs"],
            expected_obligations = 13,
            tags = ["rust", "verus"],
            size = "small",
            timeout = "short",
            target_compatible_with = ["@platforms//cpu:x86_64"],
            exec_compatible_with = ["@platforms//os:linux", "@platforms//cpu:x86_64"],
            exec_properties = {"EstimatedCPU": "1", "EstimatedComputeUnits": "1", "EstimatedMemory": "1GB"},
        )
        verus_test(
            name = "verus_budget_cutoff_witness",
            src = "proofs/budget_cutoff_witness.rs",
            srcs = ["src/budget/cutoff.rs"],
            expected_obligations = 2,
            tags = ["rust", "verus"],
            size = "small",
            timeout = "short",
            target_compatible_with = ["@platforms//cpu:x86_64"],
            exec_compatible_with = ["@platforms//os:linux", "@platforms//cpu:x86_64"],
            exec_properties = {"EstimatedCPU": "1", "EstimatedComputeUnits": "1", "EstimatedMemory": "1GB"},
        )
        verus_test(
            name = "verus_reviewer",
            src = "proofs/reviewer.rs",
            srcs = ["src/reviewer/decision.rs"],
            expected_obligations = 8,
            tags = ["rust", "verus"],
            size = "small",
            timeout = "short",
            target_compatible_with = ["@platforms//cpu:x86_64"],
            exec_compatible_with = ["@platforms//os:linux", "@platforms//cpu:x86_64"],
            exec_properties = {"EstimatedCPU": "1", "EstimatedComputeUnits": "1", "EstimatedMemory": "1GB"},
        )
        verus_test(
            name = "verus_postmortem",
            src = "proofs/postmortem.rs",
            srcs = ["src/postmortem_reply/decision.rs"],
            expected_obligations = 8,
            tags = ["rust", "verus"],
            size = "small",
            timeout = "short",
            target_compatible_with = ["@platforms//cpu:x86_64"],
            exec_compatible_with = ["@platforms//os:linux", "@platforms//cpu:x86_64"],
            exec_properties = {"EstimatedCPU": "1", "EstimatedComputeUnits": "1", "EstimatedMemory": "1GB"},
        )
        verus_test(
            name = "verus_classifier",
            src = "proofs/classifier.rs",
            srcs = ["src/classifier/decision.rs"],
            expected_obligations = 6,
            tags = ["rust", "verus"],
            size = "small",
            timeout = "short",
            target_compatible_with = ["@platforms//cpu:x86_64"],
            exec_compatible_with = ["@platforms//os:linux", "@platforms//cpu:x86_64"],
            exec_properties = {"EstimatedCPU": "1", "EstimatedComputeUnits": "1", "EstimatedMemory": "1GB"},
        )
    if native.package_name() == "crates/github":
        verus_test(
            name = "verus_status",
            src = "proofs/status.rs",
            srcs = ["src/status.rs"],
            expected_obligations = 7,
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

    if native.package_name() == "crates/controller-core":
        # Group G1: production Queue residue, with separate unbounded scans.
        translate_groups = {
            "repeated": ("translate#repeated", 2),
            "dispatch": ("translate#dispatch", 1),
            "clear": ("translate#clear_reported", 2),
            "previous": ("translate#previous_commit", 2),
            "carry": ("translate#carry_plan", 1),
            "head_current": ("translate#head_current", 1),
            "enqueue": ("translate#enqueue_pending", 1),
            "witnesses": ("translate_witnesses", 6),
        }
        for name, (module, count) in translate_groups.items():
            verus_test(
                name = "verus_translate_" + name,
                src = "verus/translate.rs",
                srcs = ["src/translate.rs"],
                specs = ["verus/translate_witnesses.rs"],
                expected_obligations = count,
                module = module,
                tags = ["rust", "verus"],
                size = "small",
                timeout = "short",
                target_compatible_with = ["@platforms//cpu:x86_64"],
                exec_compatible_with = ["@platforms//os:linux", "@platforms//cpu:x86_64"],
                exec_properties = {"EstimatedCPU": "1", "EstimatedComputeUnits": "1", "EstimatedMemory": "1GB"},
            )

    if native.package_name() == "crates/controller-core":
        # S-once phase 1: real receipt admission and non-vacuous dispatch.
        for name, module, count in [
            ("guard", "translate#begin", 1),
            ("guard_witnesses", "sonce_guard_witnesses", 2),
        ]:
            verus_test(
                name = "verus_sonce_" + name,
                src = "verus/sonce_guard.rs",
                srcs = ["src/translate.rs"],
                specs = ["verus/sonce_guard_witnesses.rs"],
                expected_obligations = count,
                module = module,
                tags = ["rust", "verus"],
                size = "small",
                timeout = "short",
                target_compatible_with = ["@platforms//cpu:x86_64"],
                exec_compatible_with = ["@platforms//os:linux", "@platforms//cpu:x86_64"],
                exec_properties = {"EstimatedCPU": "1", "EstimatedComputeUnits": "1", "EstimatedMemory": "1GB"},
            )

    if native.package_name() == "crates/controller-core":
        # Group G2b: only WorkerReply/WorkerFailed translation decisions.
        translate_worker_groups = {
            "role": ("translate_worker#valid_role", 1),
            "failed": ("translate_worker#worker_failed", 1),
            "maker": ("translate_worker#maker_self_check", 1),
            "budget": ("translate_worker#findings_prefix", 2),
            "document": ("translate_worker#document", 1),
            "review": ("translate_worker#review_route", 1),
            "witnesses": ("translate_worker_witnesses", 6),
        }
        for name, (module, count) in translate_worker_groups.items():
            verus_test(
                name = "verus_translate_worker_" + name,
                src = "verus/translate_worker.rs",
                srcs = ["src/translate_worker.rs"],
                specs = ["verus/translate_worker_witnesses.rs"],
                expected_obligations = count,
                module = module,
                tags = ["rust", "verus"],
                size = "small",
                timeout = "short",
                target_compatible_with = ["@platforms//cpu:x86_64"],
                exec_compatible_with = ["@platforms//os:linux", "@platforms//cpu:x86_64"],
                exec_properties = {"EstimatedCPU": "1", "EstimatedComputeUnits": "1", "EstimatedMemory": "1GB"},
            )

    # #836 RED matching: independent of the other wave-B proof inventories.
    if native.package_name() == "crates/harness-core":
        verus_test(
            name = "verus_red_match",
            src = "verus/red_match.rs",
            srcs = ["src/red_match.rs"],
            specs = native.glob(["verus/red_match*.rs"]),
            expected_obligations = 24,
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
            "kernel::dependencies::delivered": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::dependencies::prerequisites_delivered": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::dependencies::edges_indexable": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::dependencies::dependency_checked": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::dependencies::dependencies_checked": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::dependencies::same_declaration": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::dependencies::declarations_unique": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::dependencies::dependency_frame": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::dependencies::delivery_input": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::dependencies::delivery_changed": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::tests::scan_budget_checks_the_actual_u32_width": "Rust unit test; absent from the Verus cfg.",
            "kernel::tests::settle_budget_reserves_all_six_internal_moves_per_open_node": "Rust unit test; absent from the Verus cfg.",
            "kernel::footprint::Footprint::any": "Frozen Kani-only constructor; absent from the Verus cfg.",
            "kernel::queries::Node::waiting_on": "Read-only presentation API outside the scoped kernel proof.",
            "kernel::queries::Node::operation_pending": "Read-only presentation API outside the scoped kernel proof.",
            "kernel::queries::Node::effect_cancelling": "Read-only presentation API outside the scoped kernel proof.",
            # Nonrecursive specification definitions have no independent query.
            "kernel::retirement_domain": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::retirement_accepted": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::initial_node": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::empty_outbox": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::ready_checked": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::question_fits": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::node_checked": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::storage_ordered": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::world_checked": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::settled_world": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::step_scope": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::step_budget": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::stopping_effect_fits": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::settle_ready": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::movable": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::settle_chain": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::scheduling_safe": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::priority_number": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::pool_precedes": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::oldest_waiter": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::world_node_count": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::node_headroom": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::world_headroom": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::arithmetic_consistent": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::retry_bounds": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::reply_bounds": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::fresh_commands": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::witness_selection": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::outside_available": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::pool_line": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::pool_order": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::successful_answer": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::successful_reply": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::chosen_for_node": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::ordinary_await": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::successful_reply_input": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::answer_progress_input": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::command_progress_input": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::progress_input": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::witness_event": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::progress_scope": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::fact_progress_input": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::retirement::phased": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::retirement::stopped": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::retirement::incident": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::retirement::node_rules": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::retirement::domain": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::retirement::holders": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::retirement::accepted": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::retirement::selected": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::retirement::fact_node": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::retirement::notices_only": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::retirement::unapproved_note": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::retirement::fact_pre": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::retirement::bounded": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::retirement::fact_frame": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::retirement::infrastructure_retry": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::ranks::resumed_phase": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::ranks::continuation_rank": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::ranks::items": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::ranks::current_item": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::ranks::holding": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::ranks::pause_due": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::ranks::pending_rank": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::ranks::base_distance": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::ranks::effect_distance": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::ranks::distance": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::ranks::node_distance": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::ranks::internal_height": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::ranks::generation_capacity": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::ranks::effect_capacity": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::ranks::ticket_capacity": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::ranks::merge_capacity": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::ranks::pool_users": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::ranks::slot_waiters": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::ranks::settle_shape": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::ranks::advance_enabled": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::ranks::advance_pre": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
            "kernel::ranks::advance_post": "Logical definition; typechecked in every Verus selection, with no standalone obligation.",
        }
        groups = {
            "kernel_clones": ("kernel#*::clone", 33),
            "kernel_credit_added_step_budget": ("kernel#CREDIT_ADDED_STEP_BUDGET", 1),
            "kernel_credit_earned_step_budget": ("kernel#CREDIT_EARNED_STEP_BUDGET", 1),
            "kernel_credit_spent_step_budget": ("kernel#CREDIT_SPENT_STEP_BUDGET", 1),
            "kernel_credit_start_step_budget": ("kernel#CREDIT_START_STEP_BUDGET", 1),
            "kernel_credit_balance": ("kernel#Credit::balance", 1),
            "kernel_credit_can_spend": ("kernel#Credit::can_spend", 1),
            "kernel_credit_fund_and_pay": ("kernel#Credit::fund_and_pay", 1),
            "kernel_credit_ledger_balance": ("kernel#Credit::ledger_balance", 1),
            "kernel_credit_pay": ("kernel#Credit::pay", 1),
            "kernel_credit_spend": ("kernel#Credit::spend", 1),
            "kernel_effect_step_budget": ("kernel#EFFECT_STEP_BUDGET", 1),
            "kernel_earlyend_outcome": ("kernel#EarlyEnd::outcome", 1),
            "kernel_effectkind_starts_operation": ("kernel#EffectKind::starts_operation", 1),
            "kernel_free_rechecks": ("kernel#FREE_RECHECKS", 1),
            "kernel_generation_step_budget": ("kernel#GENERATION_STEP_BUDGET", 1),
            "kernel_node_current": ("kernel#Node::current", 1),
            "kernel_node_new": ("kernel#Node::new", 1),
            "kernel_node_next_command_id": ("kernel#Node::next_command_id", 1),
            "kernel_node_pause_move_due": ("kernel#Node::pause_move_due", 1),
            "kernel_outbox": ("kernel#OUTBOX", 1),
            "kernel_outbox_drain_into": ("kernel#Outbox::drain_into", 2),
            "kernel_outbox_new": ("kernel#Outbox::new", 1),
            "kernel_outbox_push": ("kernel#Outbox::push", 1),
            "kernel_phase_holding": ("kernel#Phase::holding", 1),
            "kernel_phase_is_closed": ("kernel#Phase::is_closed", 1),
            "kernel_phase_is_stopping": ("kernel#Phase::is_stopping", 1),
            "kernel_phase_resume": ("kernel#Phase::resume", 1),
            "kernel_priority_rank": ("kernel#Priority::rank", 1),
            "kernel_question_step_budget": ("kernel#QUESTION_STEP_BUDGET", 1),
            "kernel_release_step": ("kernel#RELEASE_STEP", 1),
            "kernel_reserve": ("kernel#RESERVE", 1),
            "kernel_resume_phase": ("kernel#Resume::phase", 1),
            "kernel_world_counter_exhaustion": ("kernel#World::counter_exhaustion", 3),
            "kernel_world_headroom_ok": ("kernel#World::headroom_ok", 1),
            "kernel_world_index": ("kernel#World::index", 1),
            "kernel_world_ticket_step_budget": ("kernel#World::ticket_step_budget", 1),
            "kernel_agenda": ("kernel#agenda", 1),
            "kernel_checked_reserves": ("kernel#checked_reserves", 1),
            "kernel_cont_rank": ("kernel#cont_rank", 1),
            "kernel_movable_frame": ("kernel#movable_frame", 1),
            "kernel_node_counter_exhaustion": ("kernel#node_counter_exhaustion", 1),
            "kernel_notice": ("kernel#notice", 1),
            "kernel_open_prefix": ("kernel#open_prefix", 1),
            "kernel_open_prefix_compare": ("kernel#open_prefix_compare", 1),
            "kernel_open_prefix_monotone": ("kernel#open_prefix_monotone", 1),
            "kernel_operation_start": ("kernel#operation_start", 1),
            "kernel_pause_node": ("kernel#pause_node", 1),
            "kernel_rank": ("kernel#rank", 1),
            "kernel_scan_step_budget": ("kernel#scan_step_budget", 1),
            "kernel_settle_step_budget": ("kernel#settle_step_budget", 1),
            "kernel_settled_world_definition": ("kernel#settled_world_definition", 1),
            "kernel_snapshot_world": ("kernel#snapshot_world", 2),
            "kernel_take_ticket_node": ("kernel#take_ticket_node", 1),
            "kernel_world_checked_definition": ("kernel#world_checked_definition", 1),
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
            "ingest_finish_ingest": ("kernel::ingest#finish_ingest", 1),
            "ingest_finish_ingest_node": ("kernel::ingest#finish_ingest_node", 1),
            "ingest_reply_ingest": ("kernel::ingest#reply_ingest", 1),
            "work_replies": ("kernel::work_replies#reply_work", 1),
            "work_replies_plan": ("kernel::work_replies#reply_plan", 1),
            "work_replies_rebase": ("kernel::work_replies#reply_rebase", 1),
            "work_replies_recheck_head": ("kernel::work_replies#update_recheck_head", 1),
            "retries": ("kernel::retries", 7),
            "acknowledgements_confirmed": ("kernel::acknowledgements#confirmed", 1),
            "acknowledgements_confirmed_agenda": ("kernel::acknowledgements#confirmed_agenda", 1),
            "acknowledgements_delivery_failed": ("kernel::acknowledgements#delivery_failed", 1),
            "acknowledgements_effect_failed": ("kernel::acknowledgements#effect_failed", 1),
            "acknowledgements_failed_delivery_node": ("kernel::acknowledgements#failed_delivery_node", 1),
            "acknowledgements_on_ack": ("kernel::acknowledgements#on_ack", 1),
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
            "command_helpers_allowed": ("kernel::command_helpers#allowed", 1),
            "command_helpers_mark_handled": ("kernel::command_helpers#mark_handled", 1),
            "continuations": ("kernel::continuations", 1),
            "checking": ("kernel::checking", 15),
            "dependencies_check_dependencies": ("kernel::dependencies#World::check_dependencies", 5),
            "dependencies_dependencies_delivered": ("kernel::dependencies#World::dependencies_delivered", 2),
            "dependencies_dependency_fact": ("kernel::dependencies#World::dependency_fact", 1),
            "dependencies_dependency_graph_acyclic": ("kernel::dependencies#World::dependency_graph_acyclic", 2),
            "dependencies_dependency_path": ("kernel::dependencies#World::dependency_path", 7),
            "dependencies_find_dependency": ("kernel::dependencies#World::find_dependency", 2),
            "dependencies_first_undelivered": ("kernel::dependencies#World::first_undelivered", 2),
            "dependencies_prepare_dependency": ("kernel::dependencies#World::prepare_dependency", 1),
            "dependencies": ("kernel::dependencies#World::record_dependency", 1),
            "dependencies_remove_dependency": ("kernel::dependencies#World::remove_dependency", 2),
            "dependencies_withhold_dependency_credit": ("kernel::dependencies#World::withhold_dependency_credit", 1),
            "dependencies_clone": ("kernel::dependencies#Dependency::clone", 1),
            "dependencies_indexability_frame": ("kernel::dependencies#indexability_frame", 1),
            "dependencies_indexable_definition": ("kernel::dependencies#indexable_definition", 1),
            "dependencies_pending_blocks": ("kernel::dependencies#pending_blocks", 1),
            "dependencies_pending_ready": ("kernel::dependencies#pending_ready", 1),
            "dependencies_prerequisite_definition": ("kernel::dependencies#prerequisite_definition", 1),
            "dependencies_unseen_prefix": ("kernel::dependencies#unseen_prefix", 1),
            "dependencies_unseen_prefix_equal": ("kernel::dependencies#unseen_prefix_equal", 1),
            "dependencies_visit_reduces_unseen": ("kernel::dependencies#visit_reduces_unseen", 1),
            "progress": ("kernel::progress", 10),
            "scheduler": ("kernel::scheduler", 13),
            "steps": ("kernel::steps", 2),
            "settling": ("kernel::settling", 3),
            "progress_theorem": ("kernel::progress_theorem", 2),
            "witness": ("kernel::witness", 2),
            "progress_pool": ("kernel::progress_pool", 6),
            "progress_global": ("kernel::progress_global", 7),
            "ranks": ("kernel::ranks#advance_release", 1),
            "ranks_advance_admit": ("kernel::ranks#advance_admit", 1),
            "ranks_advance_blocked": ("kernel::ranks#advance_blocked", 1),
            "ranks_advance_checks": ("kernel::ranks#advance_checks", 1),
            "ranks_advance_do": ("kernel::ranks#advance_do", 1),
            "ranks_advance_finished": ("kernel::ranks#advance_finished", 1),
            "ranks_advance_idle": ("kernel::ranks#advance_idle", 1),
            "ranks_advance_node": ("kernel::ranks#advance_node", 1),
            "ranks_advance_rebase": ("kernel::ranks#advance_rebase", 1),
            "ranks_advance_slot": ("kernel::ranks#advance_slot", 1),
            "ranks_advance_start": ("kernel::ranks#advance_start", 1),
            "ranks_advance_sum_bounds": ("kernel::ranks#advance_sum_bounds", 1),
            "ranks_advance_teardown": ("kernel::ranks#advance_teardown", 1),
            "ranks_distance": ("kernel::ranks#Node::distance", 1),
            "ranks_finite_minimum": ("kernel::ranks#finite_minimum", 1),
            "ranks_local_capacities": ("kernel::ranks#local_capacities", 1),
            "ranks_node_total": ("kernel::ranks#node_total", 1),
            "ranks_open_count_update": ("kernel::ranks#open_count_update", 1),
            "ranks_open_prefix_equal": ("kernel::ranks#open_prefix_equal", 1),
            "ranks_pending": ("kernel::ranks#Node::pending", 1),
            "ranks_phase_rank": ("kernel::ranks#phase_rank", 1),
            "ranks_potential_bound": ("kernel::ranks#potential_bound", 2),
            "ranks_potential_member": ("kernel::ranks#potential_member", 1),
            "ranks_potential_prefix": ("kernel::ranks#potential_prefix", 1),
            "ranks_potential_prefix_equal": ("kernel::ranks#potential_prefix_equal", 1),
            "ranks_potential_update": ("kernel::ranks#potential_update", 1),
            "ranks_settle_rank": ("kernel::ranks#Node::settle_rank", 1),
            "ranks_settle_reserve_totals": ("kernel::ranks#settle_reserve_totals", 1),
            "ranks_sum_update": ("kernel::ranks#sum_update", 1),
            "footprint": ("kernel::footprint", 22),
            "ids": ("kernel::ids", 6),
            "controller_boundary": ("controller_boundary", 2),
            "command_headroom": ("root", 1),
            "retirement_fact_preserves_node": ("kernel::retirement#fact_preserves_node", 1),
            "retirement_fact_preserves_world": ("kernel::retirement#fact_preserves_world", 1),
            "retirement_infrastructure_frame": ("kernel::retirement#infrastructure_frame", 1),
            "retirement_infrastructure_result": ("kernel::retirement#infrastructure_result", 1),
            "retirement_merge_input_distance": ("kernel::retirement#merge_input_distance", 1),
            "retirement_node_frame": ("kernel::retirement#node_frame", 1),
            "retirement_verify_selected_apply": ("kernel::retirement#verify_selected_apply", 1),
            "retirement_witness": ("kernel::retirement_witness", 2),
            "retirement_merge_witness": ("kernel::retirement_merge_witness", 2),
            "retirement_merge_waiting_witness": ("kernel::retirement_merge_waiting_witness", 2),
            "retirement_terminal_fact_witness": ("kernel::retirement_terminal_fact_witness", 2),
            "allocation_stopping_active_distance": ("kernel::allocation_stopping_lemmas#active_distance_lower", 1),
            "allocation_stopping_distance": ("kernel::allocation_stopping_lemmas#stopping_distance", 1),
            "allocation_stopping_progress": ("kernel::allocation_stopping_lemmas#stopping_progress", 1),
            "allocation_stopping_frame": ("kernel::allocation_stopping_lemmas#stopping_frame", 1),
            "allocation_incident_frame": ("kernel::allocation_incident_lemmas#incident_frame", 1),
            "facts_transition_frame": ("kernel::facts_transition_lemmas#frame_consequences", 1),
            "allocation_incident_resumed_result": ("kernel::allocation_incident_lemmas#resumed_result", 1),
            "allocation_incident_waiting_result": ("kernel::allocation_incident_lemmas#waiting_result", 1),
            "facts_ci_failed_retry_frame": ("kernel::facts_ci_failed_lemmas#retry_frame", 1),
            "facts_ci_failed_cancellation_input": ("kernel::facts_ci_failed_lemmas#cancellation_input", 1),
            "kernel_cont_rank_ready": ("kernel#cont_rank_ready", 1),
            "events_input": ("kernel::events_lemmas#selected_input", 1),
            "events_footprints": ("kernel::events_lemmas#footprints_after", 1),
            "cleanup_teardown_progress": ("kernel::cleanup_lemmas#teardown_progress", 1),
            "cleanup_resource_distance": ("kernel::cleanup_lemmas#resource_distance", 1),
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

    if native.package_name() == "crates/controller-core":
        # D1: contracts over production decisions and concrete Q7 reachability.
        verus_test(
            name = "verus_looks",
            src = "verus/looks.rs",
            srcs = ["src/looks.rs"],
            expected_obligations = 22,
            tags = ["rust", "verus"],
            size = "small",
            timeout = "short",
            target_compatible_with = ["@platforms//cpu:x86_64"],
            exec_compatible_with = ["@platforms//os:linux", "@platforms//cpu:x86_64"],
            exec_properties = {"EstimatedCPU": "1", "EstimatedComputeUnits": "1", "EstimatedMemory": "1GB"},
        )

        # D2: look-result, quota and declared-wait production decisions.
        verus_test(
            name = "verus_looks_state",
            src = "verus/looks_state.rs",
            srcs = ["src/looks_state.rs"],
            expected_obligations = 22,
            tags = ["rust", "verus"],
            size = "small",
            timeout = "short",
            target_compatible_with = ["@platforms//cpu:x86_64"],
            exec_compatible_with = ["@platforms//os:linux", "@platforms//cpu:x86_64"],
            exec_properties = {"EstimatedCPU": "1", "EstimatedComputeUnits": "1", "EstimatedMemory": "1GB"},
        )

    # CC-P / H19: real adapter decision functions and non-vacuous protocol witnesses.
    if native.package_name() == "crates/controller":
        verus_test(
            name = "verus_prismabuild",
            src = "src/adapters/prismabuild/decide.rs",
            srcs = ["src/adapters/prismabuild/decide.rs"],
            expected_obligations = 23,
            tags = ["rust", "verus"],
            size = "small",
            timeout = "short",
            target_compatible_with = ["@platforms//cpu:x86_64"],
            exec_compatible_with = ["@platforms//os:linux", "@platforms//cpu:x86_64"],
            exec_properties = {"EstimatedCPU": "1", "EstimatedComputeUnits": "1", "EstimatedMemory": "1GB"},
        )

    if native.package_name() == "crates/controller-core":
        # E1: production publication decisions and concrete branch witnesses.
        verus_test(
            name = "verus_publication",
            src = "verus/publication.rs",
            srcs = ["src/publication.rs"],
            expected_obligations = 38,
            tags = ["rust", "verus"],
            size = "small",
            timeout = "short",
            target_compatible_with = ["@platforms//cpu:x86_64"],
            exec_compatible_with = ["@platforms//os:linux", "@platforms//cpu:x86_64"],
            exec_properties = {"EstimatedCPU": "1", "EstimatedComputeUnits": "1", "EstimatedMemory": "1GB"},
        )

    if native.package_name() == "crates/controller-core":
        # H17: real reviewer-input decisions; no wire mirror in the core.
        review_inputs_groups = {
            "bounds": ("review_inputs::bounds", 2),
            "history": ("review_inputs::history", 2),
            "selection": ("review_inputs::selection", 5),
            "witnesses": ("review_inputs_witnesses", 3),
        }
        for name, (module, count) in review_inputs_groups.items():
            verus_test(
                name = "verus_review_inputs_" + name,
                src = "verus/review_inputs.rs",
                srcs = ["src/review_inputs.rs"],
                specs = ["verus/review_inputs_witnesses.rs"],
                expected_obligations = count,
                module = module,
                tags = ["rust", "verus"],
                size = "small",
                timeout = "short",
                target_compatible_with = ["@platforms//cpu:x86_64"],
                exec_compatible_with = ["@platforms//os:linux", "@platforms//cpu:x86_64"],
                exec_properties = {"EstimatedCPU": "1", "EstimatedComputeUnits": "1", "EstimatedMemory": "1GB"},
            )
    if native.package_name() == "crates/controller-core":
        # H18: real retain/curate decisions, split by decision family.
        memory_retain_groups = {
            "qualification": ("memory_retain::qualification", 3),
            "identity": ("memory_retain::identity", 1),
            "bounds": ("memory_retain::bounds", 2),
            "classification": ("memory_retain::classification", 1),
            "completion": ("memory_retain::completion", 3),
            "witnesses": ("memory_retain_witnesses", 7),
        }
        for name, (module, count) in memory_retain_groups.items():
            verus_test(
                name = "verus_memory_retain_" + name,
                src = "verus/memory_retain.rs",
                srcs = ["src/memory_retain.rs"],
                specs = ["verus/memory_retain_witnesses.rs"],
                expected_obligations = count,
                module = module,
                tags = ["rust", "verus"],
                size = "small",
                timeout = "short",
                target_compatible_with = ["@platforms//cpu:x86_64"],
                exec_compatible_with = ["@platforms//os:linux", "@platforms//cpu:x86_64"],
                exec_properties = {"EstimatedCPU": "1", "EstimatedComputeUnits": "1", "EstimatedMemory": "1GB"},
            )

    # #819: selected-release prompt identity and cache-control policy.
    if native.package_name() == "crates/controller":
        verus_test(
            name = "verus_prompt_render",
            src = "proofs/prompt_render.rs",
            srcs = ["src/prompt_render/decision.rs"],
            expected_obligations = 4,
            tags = ["rust", "verus"],
            size = "small",
            timeout = "short",
            target_compatible_with = ["@platforms//cpu:x86_64"],
            exec_compatible_with = ["@platforms//os:linux", "@platforms//cpu:x86_64"],
            exec_properties = {"EstimatedCPU": "1", "EstimatedComputeUnits": "1", "EstimatedMemory": "1GB"},
        )
    if native.package_name() == "crates/controller-core":
        # G2e: total post-match finding selection and concrete non-vacuity witnesses.
        for name, module, count in [
            ("finding", "translate_tail#finding", 1),
            ("witnesses", "translate_tail_witnesses", 5),
        ]:
            verus_test(
                name = "verus_translate_tail_" + name,
                src = "verus/translate_tail.rs",
                srcs = ["src/translate_tail.rs"],
                specs = ["verus/translate_tail_witnesses.rs"],
                module = module,
                expected_obligations = count,
                tags = ["rust", "verus"],
                size = "small",
                timeout = "short",
                target_compatible_with = ["@platforms//cpu:x86_64"],
                exec_compatible_with = ["@platforms//os:linux", "@platforms//cpu:x86_64"],
                exec_properties = {"EstimatedCPU": "1", "EstimatedComputeUnits": "1", "EstimatedMemory": "1GB"},
            )

    # G2c: miscellaneous translate residue, over the real production predicates.
    if native.package_name() == "crates/controller-core":
        verus_test(
            name = "verus_translate_misc",
            src = "verus/translate_misc.rs",
            srcs = ["src/translate_misc.rs"],
            expected_obligations = 4,
            tags = ["rust", "verus"],
            size = "small",
            timeout = "short",
            target_compatible_with = ["@platforms//cpu:x86_64"],
            exec_compatible_with = ["@platforms//os:linux", "@platforms//cpu:x86_64"],
            exec_properties = {"EstimatedCPU": "1", "EstimatedComputeUnits": "1", "EstimatedMemory": "1GB"},
        )


    # #820 pure planner rules. No controller or worker-runtime dependency.
    if native.package_name() == "crates/plan-check":
        for group, count in [("paths", 12), ("subjects", 4)]:
            verus_test(
                name = "verus_" + group,
                src = "proofs/" + group + ".rs",
                srcs = ["src/" + group + "/decision.rs"],
                expected_obligations = count,
                tags = ["rust", "verus"],
                size = "small",
                timeout = "short",
                target_compatible_with = ["@platforms//cpu:x86_64"],
                exec_compatible_with = ["@platforms//os:linux", "@platforms//cpu:x86_64"],
                exec_properties = {"EstimatedCPU": "1", "EstimatedComputeUnits": "1", "EstimatedMemory": "1GB"},
            )
