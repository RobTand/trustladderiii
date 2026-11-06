"""Optional crate_universe resolution on BuildBuddy; modifies no committed lock."""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

root = Path.cwd()
with tempfile.TemporaryDirectory(prefix='proof902-crates-') as temporary:
    work = Path(temporary)
    (work / 'src').mkdir()
    for name in ('Cargo.toml', 'Cargo.lock'):
        shutil.copyfile(root / 'spikes/proof-rs' / name, work / name)
    (work / 'src/main.rs').write_text('fn main() {}\n')
    (work / 'BUILD.bazel').write_text('exports_files(["Cargo.toml", "Cargo.lock"])\n')
    (work / 'MODULE.bazel').write_text('''module(name = "proof_spike")
bazel_dep(name = "rules_rust", version = "0.75.0")
rust = use_extension("@rules_rust//rust:extensions.bzl", "rust")
rust.toolchain(edition = "2024", versions = ["1.99.1"])
use_repo(rust, "rust_toolchains")
register_toolchains("@rust_toolchains//:all")
crate = use_extension("@rules_rust//crate_universe:extensions.bzl", "crate")
crate.from_cargo(name = "ruff_spike", cargo_lockfile = "//:Cargo.lock", manifests = ["//:Cargo.toml"], supported_platform_triples = ["x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu"])
use_repo(crate, "ruff_spike")
''')
    env = dict(os.environ, CARGO_BAZEL_REPIN='1')
    result = subprocess.run(['bazel', '--ignore_all_rc_files', 'query', '@ruff_spike//...'], cwd=work, env=env,
                            stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=360, text=True)
    out = Path(os.environ['BUILDBUDDY_ARTIFACTS_DIRECTORY'])
    (out / 'crate-universe-resolution.log').write_text(result.stdout)
    print('CRATE_UNIVERSE_RETURN_CODE', result.returncode)
    print(result.stdout[-12000:])
    raise SystemExit(result.returncode)
