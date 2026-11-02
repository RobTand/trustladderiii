"""Derive crate_universe's synthetic root entry without changing Cargo's lockfile."""

def _cargo_feature_lock_impl(ctx):
    lock = ctx.read(ctx.attr.source)
    lock += '''
[[package]]
name = "direct-cargo-bazel-deps"
version = "0.0.1"
dependencies = ["sqlx-postgres"]
'''
    ctx.file("Cargo.lock", lock)
    ctx.file("BUILD.bazel", 'exports_files(["Cargo.lock"])\n')

cargo_feature_lock = repository_rule(
    implementation = _cargo_feature_lock_impl,
    attrs = {"source": attr.label(mandatory = True, allow_single_file = True)},
)
