#!/usr/bin/env bash
# Regenerate every derived Bazel input: pip requirements from uv.lock, crate
# BUILD files from cargo metadata, and Python BUILD files from Gazelle.
set -euo pipefail
cd "$(dirname "$0")/../.."
python3 tools/bazel/requirements.py
python3 tools/bazel/rust_builds.py
bazel run //:gazelle_python_manifest.update
bazel run //:gazelle
