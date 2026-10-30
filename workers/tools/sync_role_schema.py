"""Regenerate the protocol role enum from the registered role modules."""

import argparse
import json
from pathlib import Path
import sys


ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "src"))
from tl3_workers.roles import ROLES


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    path = ROOT / "src/tl3_workers/schemas/request.json"
    request = json.loads(path.read_text())
    if args.check:
        if request["properties"]["role"]["enum"] != list(ROLES):
            parser.exit(1, "Role enum is stale. Run tools/sync_role_schema.py.\n")
    else:
        request["properties"]["role"]["enum"] = list(ROLES)
        path.write_text(json.dumps(request, indent=2) + "\n")


if __name__ == "__main__":
    main()
