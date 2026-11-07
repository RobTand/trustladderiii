#!/usr/bin/env python3
"""Admitted preparation-only driver for the public Rust staging example.

Run through pbrun with the BuildBuddy-produced binary declared in requires-files.
This never submits the staged request or invokes a native/container client.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess


def identity(path):
    path = Path(path)
    return {"path": str(path), "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
            "bytes": path.stat().st_size}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument("binary_sha256")
    parser.add_argument("destination", type=Path)
    parser.add_argument("manifest", type=Path)
    parser.add_argument("context", type=Path)
    parser.add_argument("expected_generation")
    args = parser.parse_args()
    if identity(args.binary)["sha256"] != args.binary_sha256:
        raise RuntimeError("BuildBuddy binary digest mismatch")
    source = Path(__file__).resolve().parent.parent
    subprocess.run([str(args.binary), str(args.destination), str(args.manifest),
                    str(args.context), str(source), args.expected_generation], check=True, timeout=180)
    packet_path = args.destination / "packet.json"
    packet = json.loads(packet_path.read_bytes())
    assert packet["status"] == "HELD_NOT_RELEASED_NOT_EXECUTED"
    assert packet["runtime_qualified"] is False
    assert packet["generation"] == args.expected_generation
    # Independent reader checks every staged identity recorded by the Rust entrypoint.
    checked = []
    def verify(value):
        if isinstance(value, dict):
            if {"path", "sha256", "bytes"}.issubset(value):
                observed = identity(value["path"])
                assert observed["sha256"] == value["sha256"], value["path"]
                assert observed["bytes"] == value["bytes"], value["path"]
                checked.append(observed)
            for child in value.values():
                verify(child)
        elif isinstance(value, list):
            for child in value:
                verify(child)
    verify(packet)
    result = {"status": "PASS_PREPARATION_ONLY_HELD", "packet": identity(packet_path),
              "verified_identities": checked, "source_snapshot": packet["source_snapshot"],
              "driver": identity(__file__), "cpu_affinity": sorted(os.sched_getaffinity(0)),
              "native_calls": 0, "container_calls": 0, "runtime_qualified": False}
    result_path = args.destination / "preparation-verdict.json"
    with result_path.open("x") as stream:
        json.dump(result, stream, indent=2)
        stream.write("\n")
    print(json.dumps(result, sort_keys=True), flush=True)


if __name__ == "__main__":
    main()
