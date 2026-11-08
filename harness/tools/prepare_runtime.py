"""Prepare immutable runtime artifacts inside an admitted action; never launch a container."""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import subprocess
import sys
import time

SOURCE_ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(SOURCE_ROOT / "harness/src"))


def identity(path):
    path = Path(path)
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    return {"path": str(path), "sha256": digest.hexdigest(), "bytes": path.stat().st_size}


def write(path, value):
    with path.open("x") as target:
        path.chmod(0o600)
        json.dump(value, target, indent=2)
        target.write("\n")
        target.flush()
        os.fsync(target.fileno())


def main():
    from tl3_harness.runtime_image import prepare_runtime_profile

    os.umask(0o077)
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("destination", "cli", "cli-sha256", "binary-producer-commit", "options", "image-map",
                 "image-map-sha256", "profile", "host", "platform", "host-interpreter",
                 "container-interpreter", "podman", "systemctl", "mode-prerequisites",
                 "reservation", "arguments", "provenance", "provenance-sha256"):
        parser.add_argument("--" + name, required=True)
    parser.add_argument("--timeout", type=float, default=30)
    args = parser.parse_args()
    if not math.isfinite(args.timeout) or not 0 < args.timeout <= 300:
        parser.error("timeout must be finite and within 300 seconds")
    root = Path(args.destination).resolve()
    root.mkdir(mode=0o700)  # A partial previous attempt must never be overwritten.
    environment = {"HOME": "/home/rob", "XDG_RUNTIME_DIR": "/run/user/1000", "PATH": "/usr/bin:/bin"}
    for path, expected in ((args.cli, args.cli_sha256), (args.image_map, args.image_map_sha256),
                           (args.provenance, args.provenance_sha256)):
        if identity(path)["sha256"] != expected:
            raise ValueError("preparation input digest mismatch: " + str(path))
    commit = subprocess.check_output(["/usr/bin/git", "-C", str(SOURCE_ROOT), "rev-parse", "HEAD"],
                                     env=environment, timeout=args.timeout, text=True).strip()
    inputs = {name: identity(getattr(args, name)) for name in
              ("cli", "options", "image_map", "mode_prerequisites", "reservation", "arguments", "provenance")}
    write(root / "inputs.json", {"source_commit": commit, "binary_producer_commit": args.binary_producer_commit,
                                 "inputs": inputs, "driver": identity(__file__)})
    with Path(args.mode_prerequisites).open() as source:
        prerequisites = json.load(source)
    started = time.monotonic()
    command_path = prepare_runtime_profile(
        root / "raw", host_interpreter=args.host_interpreter, container_interpreter=args.container_interpreter,
        podman=args.podman, systemctl=args.systemctl, mode="in_job", image_map=args.image_map,
        profile=args.profile, host=args.host, platform=args.platform, environment=environment,
        producer_commit=commit, prerequisites=prerequisites, timeout=args.timeout)
    write(root / "producer.json", {"elapsed_seconds": time.monotonic() - started,
                                   "command": identity(command_path)})

    def rust(command, name):
        write(root / (name + "-command.json"), command)
        argv = [args.cli, "--config", args.options]
        started = time.monotonic()
        with (root / (name + ".stdout")).open("xb") as stdout, (root / (name + ".stderr")).open("xb") as stderr:
            try:
                reply = subprocess.run(argv, input=json.dumps(command).encode(), env=environment,
                                       stdout=stdout, stderr=stderr, timeout=args.timeout)
            except subprocess.TimeoutExpired:
                write(root / (name + "-call.json"), {"argv": argv, "timeout_seconds": args.timeout,
                      "timed_out": True, "elapsed_seconds": time.monotonic() - started})
                raise
        write(root / (name + "-call.json"), {"argv": argv, "timeout_seconds": args.timeout,
              "exit": reply.returncode, "elapsed_seconds": time.monotonic() - started})
        reply.check_returncode()
        with (root / (name + ".stdout")).open("rb") as source:
            raw = source.read(4 * 1024 * 1024 + 1)
        if len(raw) > 4 * 1024 * 1024:
            raise ValueError("Rust preparation result exceeds control byte bound")
        frames = [json.loads(line) for line in raw.splitlines() if line.strip()]
        if len(frames) != 1 or frames[0].get("kind") != "result" or frames[0]["result"]["op"] != command["op"]:
            raise ValueError("Rust preparation did not return one matching result")
        return frames[0]["result"]

    bundle = rust(json.loads(command_path.read_text()), "bundle")["bundle"]
    prepared = rust({"op": "prepare_runtime", "bundle": bundle["path"],
                     "reservation": json.loads(Path(args.reservation).read_text()),
                     "arguments": json.loads(Path(args.arguments).read_text())}, "runtime")["prepared"]
    write(root / "result.json", {"bundle": bundle, "prepared": prepared, "source_commit": commit,
          "binary_producer_commit": args.binary_producer_commit, "provenance": inputs["provenance"]})
    print(json.dumps({"result": identity(root / "result.json"), "bundle": bundle}), flush=True)


if __name__ == "__main__":
    main()
