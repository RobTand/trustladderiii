"""Replay test scenarios recorded from Rust; make no budget or reply decisions.

Each case has an ordered script of command/operation and response pairs. Facts
remain in the parity corpus so the controller's --check regenerates every reply
from the real Rust functions. Unexpected commands and extra calls fail closed.
"""
from copy import deepcopy
from functools import cache
import fcntl
import json
import os
import tempfile
from pathlib import Path
import sys
from threading import Lock


VECTORS = Path(__file__).with_name("checker-vectors.json.gz")
_LOCK = Lock()


@cache
def cases():
    import gzip

    with gzip.open(VECTORS, "rt", encoding="utf-8") as source:
        return json.load(source)


def vectors():
    return [row for rows in cases().values() for row in rows]


# Declared volatile fixture fields. Verdict inputs such as counts, status,
# selectors, resource limits and container structure must remain identical.
_GIT_FIXTURE_FIELDS = {'base_sha', 'tree', 'input_tree', 'subject_tree', 'observer_tree', 'assembled_tree'}
_TEMP_FIXTURE_ROOTS = ('/tmp/', '/buildbuddy-execroot/_tmp/', '/home/buildbuddy/',
                       '/recording/', '/recorded/', '/current-test/', '/current/')


def rebound(response, recorded, current):
    """Relocate declared fixture leaves; reject every other changed fact."""
    import base64
    import hashlib
    import re

    bindings = {}
    derived = {}

    def key(value):
        return json.dumps(value, sort_keys=True, ensure_ascii=False)

    def changed(path):
        raise RuntimeError('changed PB fixture fact: ' + '.'.join(map(str, path)))

    def bind(old, new, path):
        token = key(old)
        if token in bindings and bindings[token] != new:
            changed(path)
        bindings[token] = new

    def pair(old, new, path=()):
        if type(old) is not type(new):
            changed(path)
        if isinstance(old, dict):
            if old.keys() != new.keys():
                changed(path)
            for name in old:
                pair(old[name], new[name], (*path, name))
        elif isinstance(old, list):
            if len(old) != len(new):
                changed(path)
            for index, (before, after) in enumerate(zip(old, new)):
                pair(before, after, (*path, index))
        elif old != new:
            if isinstance(old, str) and isinstance(new, str):
                temporary = (old.startswith(_TEMP_FIXTURE_ROOTS) and new.startswith(_TEMP_FIXTURE_ROOTS)
                             and Path(old).name == Path(new).name and '..' not in Path(new).parts)
                git = (path and path[-1] in _GIT_FIXTURE_FIELDS
                       and re.fullmatch('[0-9a-f]{40}', old) and re.fullmatch('[0-9a-f]{40}', new))
                if temporary or git or (key(old), key(new)) in derived:
                    bind(old, new, path)
                    return
            if path[-2:] in (('stdout', 'bytes'), ('stdout', 'sha256')) and (key(old), key(new)) in derived:
                bind(old, new, path)
                return
            changed(path)
        elif isinstance(old, str):
            bind(old, new, path)

    # Only JSON log leaves may vary. Prefixes, line counts, field sets and all
    # nonvolatile values are compared before admitting the encoded-log copy.
    if recorded.get('raw') != current.get('raw'):
        try:
            before = base64.b64decode(recorded['raw'], validate=True)
            after = base64.b64decode(current['raw'], validate=True)
            old_lines, new_lines = before.splitlines(), after.splitlines()
            if len(old_lines) != len(new_lines):
                changed(('raw',))
            for old, new in zip(old_lines, new_lines):
                if old == new:
                    continue
                index, offset = old.index(b'{'), new.index(b'{')
                if old[:index] != new[:offset]:
                    changed(('raw',))
                pair(json.loads(old[index:]), json.loads(new[offset:]), ('raw',))
            derived[key(recorded['raw']), key(current['raw'])] = True
            derived[key(hashlib.sha256(before).hexdigest()), key(hashlib.sha256(after).hexdigest())] = True
            derived[key(len(before)), key(len(after))] = True
            bind(recorded['raw'], current['raw'], ('raw',))
        except (KeyError, ValueError, UnicodeError):
            changed(('raw',))
    pair(recorded, current)

    def replace(value):
        if isinstance(value, dict):
            return {name: replace(item) for name, item in value.items()}
        if isinstance(value, list):
            return [replace(item) for item in value]
        return deepcopy(bindings.get(key(value), value))

    return replace(response)

def prepare_replay(case, cursor):
    """Decode the corpus in the fixture process, never in each checker child."""
    script = [{"command": row["command"],
               "document": row["document"] if row["command"][:1] in (["check-harness"], ["check-pb"])
               else {"kind": row["document"].get("kind")},
               "response": row["response"]} for row in cases().get(case, [])]
    cursor.with_name(cursor.name + ".script.json").write_text(
        json.dumps({"case": case, "script": script}, ensure_ascii=False), encoding="utf-8")


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


def replay(case, cursor, command, document, *, script=None):
    """A unit fake follows its script; Rust checks the original facts elsewhere."""
    # Parent threads and separately launched checker processes share one cursor.
    with _LOCK, cursor.with_suffix('.lock').open('a') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        position = int(cursor.read_text()) if cursor.exists() else 0
        if script is None:
            prepared = cursor.with_name(cursor.name + ".script.json")
            if prepared.exists():
                recording = json.loads(prepared.read_text(encoding="utf-8"))
                if recording["case"] != case:
                    raise RuntimeError(f"wrong Rust checker recording for {case}")
                script = recording["script"]
            else:
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
        response = (rebound(row["response"], row["document"], document)
                    if command[0] == "check-pb" else deepcopy(row["response"]))
        temporary = None
        try:
            with tempfile.NamedTemporaryFile(mode="w", dir=cursor.parent,
                                             prefix=cursor.name + ".", delete=False) as target:
                temporary = Path(target.name)
                target.write(str(position + 1))
                target.flush()
                os.fsync(target.fileno())
            os.replace(temporary, cursor)
        finally:
            if temporary is not None:
                temporary.unlink(missing_ok=True)

        return response


def record(case, temporary, monkeypatch):
    """Capture a unit scenario, including its child workers, with an explicit oracle."""
    import hashlib
    import subprocess

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
    if sys.argv[1:2] in (["--p5-route"], ["--p5-check"], ["--p5-vectors"]):
        return p5_main()
    if len(sys.argv) >= 4 and sys.argv[1] == "--record":
        import gzip
        import subprocess

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
        import gzip
        import subprocess

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
    if len(sys.argv) >= 6 and sys.argv[1] == "--replay-file":
        result = replay(sys.argv[3], Path(sys.argv[4]), sys.argv[5:], json.load(sys.stdin),
                        script=json.loads(Path(sys.argv[2]).read_text()))
        print(json.dumps(result, ensure_ascii=False))
        return
    if len(sys.argv) >= 5 and sys.argv[1] == "--replay":
        result = replay(sys.argv[2], Path(sys.argv[3]), sys.argv[4:], json.load(sys.stdin))
        print(json.dumps(result, ensure_ascii=False))
        return
    raise RuntimeError("usage: checker_stub.py --check/--generate <oracle> or --replay-file <script> <case> <cursor> <command>")


# P5 input and quota scripts use a separate cursor from reply/budget scripts.
P5_VECTORS = Path(__file__).with_name("p5-checker-vectors.json.xz")
P5_COMMANDS = {"check-inputs", "check-quota"}


@cache
def p5_cases():
    import lzma

    if not P5_VECTORS.exists():
        return {}
    with lzma.open(P5_VECTORS, "rt", encoding="utf-8") as handle:
        return json.load(handle)


def p5_replay(case, cursor, command, document):
    prepared = cursor.with_name(cursor.name + ".script.json")
    with _LOCK:
        recording = json.loads(prepared.read_text(encoding="utf-8"))
        if recording["case"] != case:
            raise RuntimeError(f"wrong P5 checker recording for {case}")
        script = recording["script"]
        index = int(cursor.read_text()) if cursor.exists() else 0
        if index >= len(script) or script[index]["command"] != command:
            raise RuntimeError(f"unrecorded P5 checker call in {case}: {command}")
        row = script[index]
        if row["document"].get("kind") != document.get("kind"):
            raise RuntimeError(f"out-of-order P5 operation in {case}")
        temporary = None
        try:
            with tempfile.NamedTemporaryFile(mode="w", dir=cursor.parent,
                                             prefix=cursor.name + ".", delete=False) as target:
                temporary = Path(target.name)
                target.write(str(index + 1))
                target.flush()
                os.fsync(target.fileno())
            os.replace(temporary, cursor)
        finally:
            if temporary is not None:
                temporary.unlink(missing_ok=True)
        return deepcopy(row["response"])


def p5_install(request, temporary, monkeypatch):
    import subprocess

    real = os.environ.get("TEST_REAL_CHECKER") == "1"
    recording = os.environ.get("TEST_RECORD_P5_DIR")
    if real and not recording:
        yield
        return
    delegate = os.environ["TL3_BINARY"]
    oracle = os.environ.get("TEST_P5_BINARY", delegate)
    case = request.node.nodeid
    cursor = request.getfixturevalue("checker_state_directory") / "p5-cursor"
    if not recording:
        script = [{"command": row["command"],
                   "document": {"kind": row["document"].get("kind")},
                   "response": row["response"]} for row in p5_cases().get(case, [])]
        cursor.with_name(cursor.name + ".script.json").write_text(
            json.dumps({"case": case, "script": script}, ensure_ascii=False), encoding="utf-8")
    capture = temporary / "p5-records.jsonl"
    router = temporary / "p5-checker"
    router.write_text(
        f"#!{sys.executable} -IS\nimport runpy, sys\n"
        f"sys.argv[1:1] = ['--p5-route', {case!r}, {str(cursor)!r}, {delegate!r}, {oracle!r}, {str(capture) if recording else ''!r}]\n"
        f"runpy.run_path({str(Path(__file__).resolve())!r}, run_name='__main__')\n")
    router.chmod(0o755)
    monkeypatch.setenv("TL3_BINARY", str(router))
    original = subprocess.run

    def run(command, *args, **kwargs):
        if isinstance(command, (list, tuple)) and command and command[0] == str(router):
            if command[1] not in P5_COMMANDS:
                return original([delegate, *command[1:]], *args, **kwargs)
            if recording:
                result = original([oracle, *command[1:]], *args, **kwargs)
                if result.returncode == 0:
                    with capture.open('a') as handle:
                        handle.write(json.dumps({'command': list(command[1:]),
                            'document': json.loads(kwargs['input']),
                            'response': json.loads(result.stdout)}) + '\n')
                return result
            try:
                result = p5_replay(case, cursor, list(command[1:]), json.loads(kwargs["input"]))
                code, stdout, stderr = 0, json.dumps(result), ""
            except RuntimeError as error:
                code, stdout, stderr = 1, "", str(error)
            if not (kwargs.get("text") or kwargs.get("encoding")):
                stdout, stderr = stdout.encode(), stderr.encode()
            if code and kwargs.get("check"):
                raise subprocess.CalledProcessError(code, command, output=stdout, stderr=stderr)
            return subprocess.CompletedProcess(command, code, stdout=stdout, stderr=stderr)
        return original(command, *args, **kwargs)

    monkeypatch.setattr(subprocess, "run", run)
    yield
    if recording:
        rows = [json.loads(line) for line in capture.read_text().splitlines()] if capture.exists() else []
        import hashlib
        Path(recording, hashlib.sha256(case.encode()).hexdigest() + ".json").write_text(json.dumps({case: rows}))
    else:
        consumed = int(cursor.read_text()) if cursor.exists() else 0
        assert consumed == len(p5_cases().get(case, [])), f"unconsumed P5 checker calls in {case}"


def p5_main():
    import subprocess

    if sys.argv[1:2] == ["--p5-route"]:
        case, cursor, delegate, oracle, capture, *command = sys.argv[2:]
        data = sys.stdin.buffer.read()
        if command[0] in P5_COMMANDS and not capture:
            print(json.dumps(p5_replay(case, Path(cursor), command, json.loads(data))))
            return
        binary = oracle if command[0] in P5_COMMANDS else delegate
        result = subprocess.run([binary, *command], input=data, capture_output=True)
        if capture and command[0] in P5_COMMANDS and result.returncode == 0:
            with Path(capture).open("a") as handle:
                handle.write(json.dumps({"command": command, "document": json.loads(data),
                                         "response": json.loads(result.stdout)}) + "\n")
        sys.stdout.buffer.write(result.stdout)
        sys.stderr.buffer.write(result.stderr)
        raise SystemExit(result.returncode)
    if sys.argv[1:] == ["--p5-vectors"]:
        print(json.dumps([row for rows in p5_cases().values() for row in rows]))
        return
    if sys.argv[1:2] == ["--p5-check"]:
        rows = [row for script in p5_cases().values() for row in script]
        assert rows, "P5 checker recordings are empty"
        result = subprocess.run([sys.argv[2]], input=json.dumps(rows),
                                text=True, capture_output=True, check=True)
        assert json.loads(result.stdout) == rows, "P5 Rust checker recordings drifted"
        return
    raise RuntimeError("usage: p5_stub.py --p5-check <tl3> or --p5-vectors")


if __name__ == "__main__":
    main()
