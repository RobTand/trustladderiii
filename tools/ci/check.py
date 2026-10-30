"""Light CI checks. Development runs of this script belong in PrismaBuild."""

import argparse
import json
from pathlib import Path
import shlex
import subprocess
import sys
import tomllib


ROOT = Path(__file__).resolve().parents[2]


def run(*command, cwd=ROOT, capture=False):
    print(f"{cwd.relative_to(ROOT) or '.'}: {shlex.join(map(str, command))}", flush=True)
    result = subprocess.run(
        list(map(str, command)), cwd=cwd, check=True, text=True,
        stdout=subprocess.PIPE if capture else None,
    )
    return result.stdout


def tracked_files():
    return [Path(name) for name in run("git", "ls-files", "-z", capture=True).split("\0") if name]


def active(path):
    # Spikes and deliberately broken fixtures aren't production projects.
    return not {"spikes", "fixtures"}.intersection(path.parts)


def rust_manifests(files):
    return sorted(
        path for path in files
        if path.name == "Cargo.toml" and active(path)
    )


def rust(files):
    projects = set()
    for manifest in rust_manifests(files):
        metadata = json.loads(run(
            "cargo", "metadata", "--no-deps", "--format-version", "1",
            "--manifest-path", manifest, capture=True,
        ))
        projects.add(Path(metadata["workspace_root"]).relative_to(ROOT) / "Cargo.toml")
    if not projects:
        raise RuntimeError("No Rust projects discovered")
    for manifest in sorted(projects):
        run("cargo", "fmt", "--manifest-path", manifest, "--all", "--", "--check")
        run("cargo", "clippy", "--manifest-path", manifest, "--workspace",
            "--all-targets", "--locked", "--", "-D", "warnings")
        run("cargo", "test", "--manifest-path", manifest, "--workspace",
            "--all-targets", "--locked")


def python_projects(files):
    projects = {}
    for manifest in files:
        if manifest.name == "pyproject.toml" and active(manifest):
            config = tomllib.loads((ROOT / manifest).read_text())
            if "project" not in config:
                raise RuntimeError(f"{manifest}: missing [project] table")
            projects[manifest.parent] = config
    return projects


def standalone_test_dirs(files, projects):
    return sorted({
        path.parent for path in files
        if active(path) and path.match("test_*.py")
        and not any(path.is_relative_to(project) for project in projects)
    })


def python(files):
    projects = python_projects(files)
    sources = sorted(path for path in files if path.suffix == ".py" and active(path))
    run("uv", "tool", "run", "--from", "ruff==0.14.14", "ruff", "check",
        "--isolated", "--select", "E9,F", *sources)
    for directory, config in sorted(projects.items()):
        if not any(path.match("test_*.py") and path.is_relative_to(directory) for path in files):
            # Deployment environments pin their own interpreter and have no tests to run.
            if (ROOT / directory / "uv.lock").exists():
                run("uv", "lock", "--check", "--project", ".", cwd=ROOT / directory)
            continue
        command = ["uv", "run", "--project", ".", "--python", sys.executable]
        if (ROOT / directory / "uv.lock").exists():
            command.append("--locked")
        if "test" in config["project"].get("optional-dependencies", {}):
            command.extend(["--extra", "test"])
        command.extend(["--with", "pytest==9.0.2", "python", "-m", "pytest", "--durations=15"])
        # Nested projects own their files and run with their own dependencies.
        command.extend(
            f"--ignore={nested.relative_to(directory)}"
            for nested in sorted(projects)
            if nested != directory and nested.is_relative_to(directory)
        )
        # Run in the package so pytest reads its own configuration and imports.
        run(*command, cwd=ROOT / directory)
    for directory in standalone_test_dirs(files, projects):
        run(sys.executable, "-m", "unittest", "discover", "-s", directory,
            "-p", "test_*.py", "-v")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("language", choices=("rust", "python"))
    args = parser.parse_args()
    files = tracked_files()
    {"rust": rust, "python": python}[args.language](files)


if __name__ == "__main__":
    main()
