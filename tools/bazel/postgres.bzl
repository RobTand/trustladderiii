"""Unpack the checked Zonky archive as an immutable Bazel repository input."""

def _postgres_archive_impl(ctx):
    ctx.download(ctx.attr.url, "postgres.jar", sha256 = ctx.attr.sha256)
    ctx.extract("postgres.jar", output = "jar", type = "zip")
    archives = [p for p in ctx.path("jar").readdir() if p.basename.endswith(".txz")]
    if len(archives) != 1:
        fail("Expected exactly one Postgres archive in Zonky jar")
    ctx.extract(archives[0], type = "tar.xz")
    ctx.delete("jar")
    ctx.delete("postgres.jar")
    ctx.file("BUILD.bazel", '''package(default_visibility = ["//visibility:public"])
exports_files(["bin/postgres"])
filegroup(name = "all", srcs = glob(["bin/**", "lib/**", "share/**"]))
''')

postgres_archive = repository_rule(
    implementation = _postgres_archive_impl,
    attrs = {"url": attr.string(mandatory = True), "sha256": attr.string(mandatory = True)},
)
