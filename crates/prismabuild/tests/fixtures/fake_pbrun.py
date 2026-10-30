"""Local process fixture. No PrismaBuild imports, submission, or withdrawal."""
import json
import pathlib
import sys

root = pathlib.Path(sys.argv[1])
with (root / "calls.jsonl").open("a") as stream:
    stream.write(json.dumps(sys.argv[2:]) + "\n")
sys.stdout.write((root / "response.json").read_text())
sys.exit(int((root / "exit_code").read_text()))
