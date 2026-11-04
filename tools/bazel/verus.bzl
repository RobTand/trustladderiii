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
exec "%s" "%s" "%s" "%s" "%s" "%s" "%s"
""" % (
        ctx.workspace_name,
        ctx.executable._runner.short_path,
        toolchain.verifier.short_path,
        toolchain.z3.short_path,
        toolchain.sysroot.short_path,
        ctx.file.src.short_path,
        ctx.attr.expected_obligations,
        ctx.label,
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
