"""Copy selected real queue records and attempt links, replacing broker secrets.

Run explicitly to refresh fixtures; tests never access the real queue.
"""

import argparse
import json
from pathlib import Path
import re


RECORDS = {
    "done": "473655276a87335e1b1f82fcfec15c5b2beef4f5852b1d133e52b676bb3ec364",
    "failed": "b64bddc12fdf7126ace2a4ed2ea258bfcad133e253c131e70d222e346262b55e",
    "withdrawn": "1c3b13fd5b00f2c7ac79b1e97afbd1edafa93ea2e6ea0a4d44005a484eb9a20b",
}


def sanitize(document):
    secrets = {}

    def collect(value):
        if isinstance(value, dict):
            for name, child in value.items():
                if isinstance(child, str) and child and (
                    "token" in name.lower() or "nonce" in name.lower()
                ):
                    secrets[child] = "fake-nonce" if "nonce" in name.lower() else "fake-token"
                collect(child)
        elif isinstance(value, list):
            for index, child in enumerate(value):
                if child in ("--token", "--nonce") and index + 1 < len(value):
                    secrets[str(value[index + 1])] = "fake-" + child[2:]
                collect(child)
        elif isinstance(value, str):
            for match in re.finditer(r"--(token|nonce)(?:=|[\s'\",]+)([\w.-]+)", value):
                secrets[match[2]] = "fake-" + match[1]

    def replace(value):
        if isinstance(value, dict):
            return {name: replace(child) for name, child in value.items()}
        if isinstance(value, list):
            return [replace(child) for child in value]
        if isinstance(value, str):
            for secret in sorted(secrets, key=len, reverse=True):
                value = value.replace(secret, secrets[secret])
        return value

    collect(document)
    return replace(document)


def capture(root, output):
    def copy(relative):
        value = sanitize(json.loads((root / relative).read_bytes()))
        target = output / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")
        return value

    for state, key in RECORDS.items():
        record = copy(Path(state) / f"{key}.json")
        history = record.get("withdrawn_attempt", record).get("attempt_history", [])
        for link in history:
            relative = Path(link["outcome"])
            if relative.is_absolute() or ".." in relative.parts:
                raise ValueError("unsafe attempt link")
            copy(relative)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, required=True)
    parser.add_argument("--output", type=Path, default=Path(__file__).parent / "queue")
    args = parser.parse_args()
    capture(args.root, args.output)
