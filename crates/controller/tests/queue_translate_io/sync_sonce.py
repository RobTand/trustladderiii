#!/usr/bin/env python3
"""Refresh the compiled Queue copies from Git and production; never run tests."""

import argparse
from pathlib import Path
import subprocess


BASE = "f5b0ba2fb16a686232c08fe5cee907f1d43c5258"
START = "            Observation::Queue(mut fact) => {"
END = "            Observation::GitHubComment {"


def arm(source):
    first = source.index(START)
    return source[first:source.index(END, first)]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--reference", action="store_true", help="restore pinned old sources")
    args = parser.parse_args()
    directory = Path(__file__).resolve().parent
    repo = directory.parents[3]
    current = directory / "current.rs"
    wrapper = current.read_text()
    prefix = wrapper[:wrapper.index(START)]
    suffix = wrapper[wrapper.index("\n        };\n", wrapper.index(START)) + 1:]
    production = (repo / "crates/controller/src/lib.rs").read_text()
    current.write_text(prefix + arm(production) + suffix)
    if args.reference:
        def blob(path):
            return subprocess.check_output(["git", "-C", str(repo), "show", f"{BASE}:{path}"], text=True)

        old = arm(blob("crates/controller/src/lib.rs"))
        (directory / "base-queue.rs.txt").write_text(old)
        (directory / "reference.rs").write_text(
            "use crate::reference_core as tl3_controller_core;\n" + prefix + old + suffix
        )
        core = blob("crates/controller-core/src/translate.rs")
        (directory / "reference_core.rs").write_text("pub mod translate {\n" + core + "\n}\n")


if __name__ == "__main__":
    main()
