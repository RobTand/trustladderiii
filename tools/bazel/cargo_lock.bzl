"""Derive crate_universe's synthetic root entry without changing Cargo's lockfile."""

def _cargo_feature_lock_impl(ctx):
    lock = ctx.read(ctx.attr.source)
    # Cargo rewrites this synthetic entry in place. Emit its normalized layout
    # before crate_universe records the input digest, including on fresh runners.
    entry = '''[[package]]
name = "direct-cargo-bazel-deps"
version = "0.0.1"
dependencies = [
 "sqlx-postgres",
]

'''
    packages = lock.split("[[package]]\n")
    normalized = packages[0]
    inserted = False
    for package in packages[1:]:
        if not inserted and package.split("\n")[0] > 'name = "direct-cargo-bazel-deps"':
            normalized += entry
            inserted = True
        normalized += "[[package]]\n" + package
    if not inserted:
        normalized += entry
    ctx.file("Cargo.lock", normalized)
    ctx.file("BUILD.bazel", 'exports_files(["Cargo.lock"])\n')

cargo_feature_lock = repository_rule(
    implementation = _cargo_feature_lock_impl,
    attrs = {"source": attr.label(mandatory = True, allow_single_file = True)},
)
