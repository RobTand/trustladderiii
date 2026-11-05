#!/usr/bin/env bash
set -euo pipefail
# Exploratory driver: invoke only inside a PrismaBuild admitted action.
cd "$(dirname "$0")/../../.."
work=$(mktemp -d "$PWD/.verus-toolchain-XXXXXXXX")
url=https://github.com/verus-lang/verus/releases/download/release/0.2026.11.02.8bf68ee/verus-0.2026.11.02.8bf68ee-x86-linux.zip
curl --fail --silent --show-error --location "$url" --output "$work/verus.zip"
printf '%s  %s\n' f3a0119292df8855e4435da8130bee1873b0e6a60c306accf81a08ea687bba3c "$work/verus.zip" | sha256sum --check
unzip -q "$work/verus.zip" -d "$work"
chmod +x "$work/verus-x86-linux/"{verus,rust_verify,z3}
printf '%s  %s\n' e583c4186a45e72411fa2cb2048401eed03f0f8e5f24694676a8f6271a50b765 "$work/verus-x86-linux/z3" | sha256sum --check
# Build a private sysroot from pinned official components. Execute no downloaded installer.
mkdir "$work/rust"
for component in rustc rustc-dev rust-std; do
  mkdir "$work/$component-download"
  archive="$work/$component-download/$component.tar.xz"
  curl --fail --silent --show-error --location "https://static.rust-lang.org/dist/2026-10-15/$component-1.99.1-x86_64-unknown-linux-gnu.tar.xz" --output "$archive"
  case "$component" in
    rustc) digest=2ff05ff9c08a90861e7f6e4065b40a8b87463b4576186fd613871c3e37f2091f ;;
    rustc-dev) digest=e2b1c9ffe50910d5274339b52774eec05433682835219566449cd09c67c1d1d2 ;;
    rust-std) digest=109b074225bf5b9ea3820890876bfdf4ed161adbfaf94b2b4b5984c85ee5d958 ;;
  esac
  printf '%s  %s\n' "$digest" "$archive" | sha256sum --check
  tar -xJf "$archive" -C "$work/$component-download"
  dist="$work/$component-download/$component-1.99.1-x86_64-unknown-linux-gnu"
  payload="$component"
  if [[ $component == rust-std ]]; then payload=rust-std-x86_64-unknown-linux-gnu; fi
  cp -a "$dist/$payload/." "$work/rust/"
done
export VERUS_USE_RUSTUP=0
export PATH="$work/rust/bin:/usr/local/bin:/usr/bin:/bin"
export LD_LIBRARY_PATH="$work/rust/lib"

export VERUS_Z3_PATH="$work/verus-x86-linux/z3"
"$work/verus-x86-linux/verus" --version
"$VERUS_Z3_PATH" --version
"$work/rust/bin/rustc" --version
if [[ ${1:-} == --modules ]]; then
  shift
  status=0
  for module in "$@"; do
    printf 'PROOF_MODULE %s\n' "$module"
    /usr/bin/time -v "$work/verus-x86-linux/verus" --num-threads "${TL3_VERUS_THREADS:-1}" --rlimit 30 --multiple-errors 0 -V spinoff-all --sysroot "$work/rust" crates/kernel/verus/progress.rs --edition 2024 --verify-only-module "$module" --time --output-json --smt-option "timeout=${TL3_VERUS_QUERY_TIMEOUT_MS:-10000}" || status=1
  done
  exit "$status"
elif [[ ${1:-} == --mutants ]]; then
  shift
  /usr/bin/time -v python3 crates/kernel/tools/progress_mutants.py "$work/verus-x86-linux/verus" "$work/rust" "$@"
else
  /usr/bin/time -v "$work/verus-x86-linux/verus" --num-threads "${TL3_VERUS_THREADS:-1}" --rlimit 30 --multiple-errors 0 -V spinoff-all --sysroot "$work/rust" "${1:-crates/kernel/verus/progress.rs}" "${@:2}"
fi
