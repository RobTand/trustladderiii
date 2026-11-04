"""A separate offline Verus distribution, resolved before test execution."""

load("//tools/bazel:verus_pins.bzl", "RUST_DATE", "RUST_SHA256", "RUST_VERSION", "VERUS_SHA256", "VERUS_URL", "VERUS_VERSION", "Z3_SHA256")

VerusToolchainInfo = provider(fields = ["verifier", "z3", "sysroot", "files"])

def _repository_impl(ctx):
    ctx.download_and_extract(
        ctx.attr.url,
        output = "verus",
        sha256 = ctx.attr.sha256,
        stripPrefix = "verus-x86-linux",
    )
    checked = ctx.execute(["sha256sum", "verus/z3"])
    if checked.return_code or checked.stdout.split()[0] != ctx.attr.z3_sha256:
        fail("Bundled Verus Z3 digest mismatch: " + checked.stderr)
    for component, digest in ctx.attr.rust_sha256.items():
        dist = "%s-%s-x86_64-unknown-linux-gnu" % (component, ctx.attr.rust_version)
        payload = "rust-std-x86_64-unknown-linux-gnu" if component == "rust-std" else component
        ctx.download_and_extract(
            "https://static.rust-lang.org/dist/%s/%s.tar.xz" % (ctx.attr.rust_date, dist),
            output = "components/" + component,
            stripPrefix = dist + "/" + payload,
            sha256 = digest,
        )
        copied = ctx.execute(["cp", "-a", str(ctx.path("components/" + component)) + "/.", str(ctx.path("rust"))])
        if copied.return_code:
            fail("Cannot assemble Verus sysroot: " + copied.stderr)
    ctx.delete("components")
    executable = ctx.execute(["chmod", "+x", "verus/verus", "verus/rust_verify", "verus/z3", "rust/bin/rustc"])
    if executable.return_code:
        fail(executable.stderr)
    ctx.file("pins.json", json.encode({
        "verus": ctx.attr.version,
        "url": ctx.attr.url,
        "sha256": ctx.attr.sha256,
        "z3": ctx.attr.z3_sha256,
        "rust": ctx.attr.rust_version,
        "rust_date": ctx.attr.rust_date,
        "components": ctx.attr.rust_sha256,
    }) + "\n")
    ctx.file("BUILD.bazel", '''load("@tl3//tools/bazel:verus_toolchain.bzl", "verus_toolchain_files")
package(default_visibility = ["//visibility:public"])
verus_toolchain_files(
    name = "toolchain",
    verifier = "verus/verus",
    z3 = "verus/z3",
    sysroot = "rust/bin/rustc",
    srcs = glob(["verus/**", "rust/**"]) + ["pins.json"],
)
''')

verus_toolchain = repository_rule(
    implementation = _repository_impl,
    attrs = {
        "version": attr.string(default = VERUS_VERSION),
        "url": attr.string(default = VERUS_URL),
        "sha256": attr.string(default = VERUS_SHA256),
        "z3_sha256": attr.string(default = Z3_SHA256),
        "rust_version": attr.string(default = RUST_VERSION),
        "rust_date": attr.string(default = RUST_DATE),
        "rust_sha256": attr.string_dict(default = RUST_SHA256),
    },
)

def _files_impl(ctx):
    files = depset(ctx.files.srcs)
    return [
        DefaultInfo(files = files),
        VerusToolchainInfo(
            verifier = ctx.file.verifier,
            z3 = ctx.file.z3,
            sysroot = ctx.file.sysroot,
            files = files,
        ),
    ]

verus_toolchain_files = rule(
    implementation = _files_impl,
    attrs = {
        "srcs": attr.label_list(allow_files = True),
        "verifier": attr.label(allow_single_file = True),
        "z3": attr.label(allow_single_file = True),
        "sysroot": attr.label(allow_single_file = True),
    },
)
