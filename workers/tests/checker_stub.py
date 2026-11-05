"""Replay test scenarios recorded from Rust; make no budget or reply decisions.

Each case has an ordered script of command/operation and response pairs. Facts
remain in the parity corpus so the controller's --check regenerates every reply
from the real Rust functions. Unexpected commands and extra calls fail closed.
"""
from copy import deepcopy
from functools import cache
import fcntl
import gzip
import json
import hashlib
import os
import tempfile
from pathlib import Path
import subprocess
import sys
from threading import Lock


VECTORS = Path(__file__).with_name("checker-vectors.json.gz")
_LOCK = Lock()


@cache
def cases():
    with gzip.open(VECTORS, "rt", encoding="utf-8") as source:
        return json.load(source)


def vectors():
    return [row for rows in cases().values() for row in rows]


def difference(expected, actual, path='$'):
    if type(expected) is not type(actual):
        return path + ': type changed'
    if isinstance(expected, dict):
        if expected.keys() != actual.keys():
            return path + ': keys changed'
        for key in expected:
            left, right = expected[key], actual[key]
            if key == 'summary_json':
                left, right = json.loads(left), json.loads(right)
            if found := difference(left, right, path + '.' + key):
                return found
    elif isinstance(expected, list):
        if len(expected) != len(actual):
            return path + ': length changed'
        for index, (left, right) in enumerate(zip(expected, actual)):
            if found := difference(left, right, path + f'[{index}]'):
                return found
    elif expected != actual:
        return f'{path}: {str(expected)[:160]!r} != {str(actual)[:160]!r}'
    return ''


def replay(case, cursor, command, document):
    """A unit fake follows its script; Rust checks the original facts elsewhere."""
    # Parent threads and separately launched checker processes share one cursor.
    with _LOCK, cursor.with_suffix('.lock').open('a') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        position = int(cursor.read_text()) if cursor.exists() else 0
        script = cases().get(case, [])
        if position >= len(script):
            raise RuntimeError(f"unrecorded Rust checker call in {case}: {command}")
        row = script[position]
        if row["command"] != command or row["document"].get("kind") != document.get("kind"):
            raise RuntimeError(f"out-of-order Rust checker call in {case} at {position}: {command}")
        if command[:1] == ['check-harness'] and (
                json.dumps(row['document'], sort_keys=True, allow_nan=False)
                != json.dumps(document, sort_keys=True, allow_nan=False)):
            raise RuntimeError(f"Rust harness document changed in {case} at {position}: "
                               + difference(row['document'], document))
        cursor.write_text(str(position + 1))
        return deepcopy(row["response"])


def record(case, temporary, monkeypatch):
    """Capture a unit scenario, including its child workers, with an explicit oracle."""
    binary = os.environ["TL3_BINARY"]
    rows = temporary / "rust-verdicts.jsonl"
    checker = temporary / "record-rust-checker"
    checker.write_text(
        f"#!{sys.executable}\n"
        "import json, subprocess, sys\n"
        "data = sys.stdin.buffer.read()\n"
        f"run = subprocess.run([{binary!r}, *sys.argv[1:]], input=data, capture_output=True)\n"
        "if run.returncode == 0:\n"
        " row = {'command': sys.argv[1:], 'document': json.loads(data), 'response': json.loads(run.stdout)}\n"
        f" with open({str(rows)!r}, 'ab', buffering=0) as target: target.write(json.dumps(row, ensure_ascii=False).encode() + b'\\n')\n"
        "sys.stdout.buffer.write(run.stdout)\n"
        "sys.stderr.buffer.write(run.stderr)\n"
        "raise SystemExit(run.returncode)\n")
    checker.chmod(0o755)
    monkeypatch.setenv("TL3_BINARY", str(checker))
    original = subprocess.run

    def recorded(command, *args, **kwargs):
        if isinstance(command, (list, tuple)) and command and command[0] == str(checker):
            result = original([binary, *command[1:]], *args, **kwargs)
            if result.returncode == 0:
                row = {"command": list(command[1:]), "document": json.loads(kwargs["input"]),
                       "response": json.loads(result.stdout)}
                with rows.open("ab", buffering=0) as target:
                    target.write(json.dumps(row, ensure_ascii=False).encode() + b"\n")
            return result
        return original(command, *args, **kwargs)

    monkeypatch.setattr(subprocess, "run", recorded)
    yield
    values = [json.loads(line) for line in rows.read_text().splitlines()] if rows.exists() else []
    destination = Path(os.environ["TEST_RECORD_CHECKER_DIR"], hashlib.sha256(case.encode()).hexdigest() + ".json")
    destination.write_text(json.dumps({case: values}, ensure_ascii=False))


def main():
    if len(sys.argv) >= 4 and sys.argv[1] == "--record":
        with tempfile.TemporaryDirectory(prefix="tl3-checker-recordings-") as directory:
            environment = {**os.environ, "TL3_BINARY": str(Path(sys.argv[2]).resolve()),
                           "TEST_REAL_CHECKER": "1", "TEST_RECORD_CHECKER_DIR": directory}
            run = subprocess.run([sys.executable, "-m", "pytest", *sys.argv[3:]],
                                 env=environment, cwd=Path(__file__).resolve().parents[1])
            corpus = dict(cases()) if VECTORS.exists() else {}
            for result in sorted(Path(directory).glob("*.json")):
                corpus.update(json.loads(result.read_text()))
            data = json.dumps(dict(sorted(corpus.items())), ensure_ascii=False, separators=(",", ":")).encode()
            VECTORS.write_bytes(gzip.compress(data, mtime=0))
            run.check_returncode()
        return
    if sys.argv[1:] == ["--vectors"]:
        json.dump(vectors(), sys.stdout, ensure_ascii=False)
        return
    if len(sys.argv) == 3 and sys.argv[1] in {"--check", "--generate"}:
        requests = [{"command": row["command"], "document": row["document"]} for row in vectors()]
        result = subprocess.run([sys.argv[2]], input=json.dumps(requests, ensure_ascii=False),
                                capture_output=True, text=True, check=True)
        generated = json.loads(result.stdout)
        if sys.argv[1] == "--check":
            if generated != vectors():
                raise RuntimeError("Rust checker recordings drifted; regenerate with --generate")
        else:
            updated = iter(generated)
            corpus = {case: [next(updated) for row in rows] for case, rows in cases().items()}
            data = json.dumps(corpus, ensure_ascii=False, separators=(",", ":")).encode()
            VECTORS.write_bytes(gzip.compress(data, mtime=0))
        return
    if len(sys.argv) >= 5 and sys.argv[1] == "--replay":
        result = replay(sys.argv[2], Path(sys.argv[3]), sys.argv[4:], json.load(sys.stdin))
        print(json.dumps(result, ensure_ascii=False))
        return
    raise RuntimeError("usage: checker_stub.py --check/--generate <oracle> or --replay <case> <cursor> <command>")


if __name__ == "__main__":
    main()
