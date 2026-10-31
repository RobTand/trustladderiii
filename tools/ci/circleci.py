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


def rust():
    shallow = check.run("git", "rev-parse", "--is-shallow-repository", capture=True).strip()
    if shallow == "true":
        check.run("git", "fetch", "--no-tags", "--unshallow", "origin")
    check.run("git", "fetch", "--no-tags", "origin", "+refs/heads/main:refs/remotes/origin/main")
    shallow = check.run("git", "rev-parse", "--is-shallow-repository", capture=True).strip()
    if shallow != "false":
        raise RuntimeError("The Rust budget gate requires full checkout history")
    baseline = check.run("git", "rev-parse", "origin/main", capture=True).strip()
    print(f"Rust budget checkout: shallow={shallow}, origin/main={baseline}", flush=True)
    report = ROOT / "target" / "ci-test-times.json"
    report.unlink(missing_ok=True)
    check.rust(check.tracked_files())
    timings = json.loads(report.read_text())
    suite = ET.Element("testsuite", name="Rust", tests=str(len(timings["tests"])),
                       time=str(timings["seconds"]))
    for identifier, elapsed in sorted(timings["tests"].items()):
        binary, _, name = identifier.partition("::")
        ET.SubElement(suite, "testcase", classname=binary, name=name, time=str(elapsed))
    ET.ElementTree(suite).write(RESULTS / "rust.xml", encoding="utf-8", xml_declaration=True)


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
    args = parser.parse_args()
    RESULTS.mkdir(exist_ok=True)
    if args.language == "rust":
        rust()
    else:
        python()


if __name__ == "__main__":
    main()
