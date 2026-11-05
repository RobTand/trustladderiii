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
        groups = {
            "kernel": ("kernel", 107),
            "events": ("kernel::events", 1),
            "allocation": ("kernel::allocation", 10),
            "reply_admission": ("kernel::reply_admission", 4),
            "ingest": ("kernel::ingest", 3),
            "work_replies": ("kernel::work_replies", 4),
            "retries": ("kernel::retries", 7),
            "acknowledgements": ("kernel::acknowledgements", 6),
            "facts": ("kernel::facts", 3),
            "commands": ("kernel::commands", 1),
            "answers": ("kernel::answers", 1),
            "command_helpers": ("kernel::command_helpers", 2),
            "continuations": ("kernel::continuations", 1),
            "checking": ("kernel::checking", 15),
            "dependencies": ("kernel::dependencies", 31),
            "ranks": ("kernel::ranks", 30),
            "footprint": ("kernel::footprint", 21),
            "ids": ("kernel::ids", 6),
            "controller_boundary": ("controller_boundary", 2),
            "command_headroom": ("root", 1),
            "retirement": ("kernel::retirement", 4),
        }
        for name, (module, count) in groups.items():
            verus_test(
                name = "verus_" + name,
                src = "verus/progress.rs",
                srcs = native.glob(["src/**/*.rs"], exclude = ["src/proofs.rs"]) + ["//crates/controller:src/kernel_boundary.rs"],
                expected_obligations = count,
                module = module,
                specs = ["verus/retirement.rs"],
                tags = ["rust", "verus"],
                size = "small",
                timeout = "short",
                target_compatible_with = ["@platforms//cpu:x86_64"],
                exec_compatible_with = ["@platforms//os:linux", "@platforms//cpu:x86_64"],
                exec_properties = {"EstimatedCPU": "1", "EstimatedComputeUnits": "1", "EstimatedMemory": "1GB"},
            )
