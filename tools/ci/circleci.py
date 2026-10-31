"""CircleCI test selection and execution. Development checks run through PrismaBuild."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import xml.etree.ElementTree as ET

import check

ROOT = check.ROOT
RESULTS = ROOT / "test-results"


def rust_filter(changed, packages, full=False):
    if full or not changed:
        return "all()"
    selected = set()
    for filename in changed:
        owners = [name for name, directory in packages.items() if Path(filename).is_relative_to(directory)]
        if not owners:
            return "all()"
        selected.update(owners)
    return " | ".join(f"rdeps(={name})" for name in sorted(selected))


def python_groups(files, projects):
    groups = {}
    for path in sorted(files):
        if not check.active(path) or not path.match("test_*.py"):
            continue
        owners = [p for p in projects if path.is_relative_to(p)]
        owner = max(owners, key=lambda p: len(p.parts)) if owners else Path(".")
        groups.setdefault(owner, []).append(path)
    return groups


def split(items, timing_type):
    if not items:
        return []
    result = subprocess.run(
        ["circleci", "tests", "split", "--split-by=timings", f"--timings-type={timing_type}"],
        input="\n".join(items) + "\n", text=True, capture_output=True, check=True,
    )
    print(result.stderr, file=sys.stderr, end="")
    return result.stdout.splitlines()


def changed_files():
    try:
        check.run("git", "fetch", "origin", "main")
        base = check.run("git", "merge-base", "HEAD", "origin/main", capture=True).strip()
        return check.run("git", "diff", "--name-only", "--no-renames", base, "HEAD", capture=True).splitlines()
    except subprocess.CalledProcessError:
        # Missing history must increase coverage, never silently select nothing.
        return []


def rust(full):
    files = check.tracked_files()
    manifests = check.rust_manifests(files)
    workspaces = {}
    for manifest in manifests:
        metadata = json.loads(check.run("cargo", "metadata", "--no-deps", "--format-version", "1", "--manifest-path", manifest, capture=True))
        workspaces[metadata["workspace_root"]] = metadata
    if not workspaces:
        raise RuntimeError("No Rust projects discovered")
    changed = [] if full else changed_files()
    for index, (directory, metadata) in enumerate(sorted(workspaces.items())):
        manifest = Path(directory) / "Cargo.toml"
        packages = {p["name"]: Path(p["manifest_path"]).parent.relative_to(ROOT) for p in metadata["packages"] if p["id"] in metadata["workspace_members"]}
        selection = rust_filter(changed, packages, full)
        print(f"Rust selection: {selection}", flush=True)
        if os.environ.get("CIRCLE_NODE_INDEX", "0") == "0":
            check.run("cargo", "fmt", "--manifest-path", manifest, "--all", "--", "--check")
            check.run("cargo", "clippy", "--manifest-path", manifest, "--workspace", "--all-targets", "--locked", "--", "-D", "warnings")
        listing = json.loads(check.run("cargo", "nextest", "list", "--manifest-path", manifest, "--workspace", "--all-targets", "--locked", "-E", selection, "--message-format", "json", capture=True))
        binaries = split(sorted(listing["rust-suites"]), "classname")
        if not binaries:
            print("No Rust binaries assigned to this node", flush=True)
            continue
        shard = " | ".join(f"binary_id(={binary})" for binary in binaries)
        report = RESULTS / f"rust-{index}.xml"
        config = ROOT / ".circleci" / "nextest.toml"
        try:
            check.run("cargo", "nextest", "run", "--manifest-path", manifest, "--config-file", config, "--profile", "circleci", "--workspace", "--all-targets", "--locked", "--no-fail-fast", "-E", f"({selection}) & ({shard})")
        finally:
            source = Path(metadata["target_directory"]) / "nextest" / "circleci" / "junit.xml"
            if source.exists():
                report.write_bytes(source.read_bytes())


def python():
    files = check.tracked_files()
    projects = check.python_projects(files)
    if os.environ.get("CIRCLE_NODE_INDEX", "0") == "0":
        sources = sorted(p for p in files if p.suffix == ".py" and check.active(p))
        check.run("uv", "tool", "run", "--from", "ruff==0.14.14", "ruff", "check", "--isolated", "--select", "E9,F", *sources)
        for directory in sorted(projects):
            if (ROOT / directory / "uv.lock").exists():
                check.run("uv", "lock", "--check", "--project", ".", cwd=ROOT / directory)
    groups = python_groups(files, projects)
    assigned = set(split([str(p) for paths in groups.values() for p in paths], "file"))
    for index, (directory, paths) in enumerate(sorted(groups.items())):
        selected = [p for p in paths if str(p) in assigned]
        if not selected:
            continue
        command = ["uv", "run", "--python", "3.14"]
        config = projects.get(directory)
        if config is None:
            command.append("--no-project")
        else:
            command.extend(["--project", "."])
            if (ROOT / directory / "uv.lock").exists():
                command.append("--locked")
            if "test" in config["project"].get("optional-dependencies", {}):
                command.extend(["--extra", "test"])
        report = RESULTS / f"python-{index}.xml"
        command.extend(["--with", "pytest==9.0.2", "python", "-m", "pytest", "--import-mode=importlib", "--durations=15", f"--junitxml={report}", "-o", "junit_family=xunit1", *[str(p.relative_to(directory)) for p in selected]])
        try:
            check.run(*command, cwd=ROOT / directory)
        finally:
            if report.exists():
                tree = ET.parse(report)
                for case in tree.iter("testcase"):
                    if "file" in case.attrib:
                        case.set("file", str(directory / case.attrib["file"]))
                tree.write(report, encoding="utf-8", xml_declaration=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("language", choices=("rust", "python"))
    parser.add_argument("--full", action="store_true")
    args = parser.parse_args()
    RESULTS.mkdir(exist_ok=True)
    if args.language == "rust":
        rust(args.full or os.environ.get("CIRCLE_BRANCH", "main") == "main" or os.environ.get("CIRCLE_BRANCH", "").startswith("mergify/merge-queue/"))
    else:
        python()


if __name__ == "__main__":
    main()
