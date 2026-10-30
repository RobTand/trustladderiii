"""Copy an admitted trial's verified payload and receipts into the maker replay."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("action_key")
parser.add_argument("--prefix", default="")
parser.add_argument("--fleet-root", type=Path, default=Path("/mnt/shared/prismabuild-fleet"))
args = parser.parse_args()
if not re.fullmatch(r"[0-9a-f]{64}", args.action_key):
    parser.error("expected a full action key")
if args.prefix not in ("", "failure-"):
    parser.error("expected an empty prefix or failure-")
key = args.action_key
receipt = json.loads((args.fleet_root / "cas/actions/v3" / key[:2] / f"{key}.json").read_text())
result = receipt["result"]
payload = (args.fleet_root / "cas/blobs" / result["sha256"][:2] / result["sha256"]).read_bytes()
if len(payload) != result["bytes"] or hashlib.sha256(payload).hexdigest() != result["sha256"]:
    raise ValueError("trial payload does not match its CAS receipt")
ending = json.loads(subprocess.check_output([
    sys.executable, str(args.fleet_root / "repo/tools/pbwait.py"), key,
    "--wait-s", "0", "--endings",
]))
terminal, = ending["endings"]
if not (ending["complete"] and terminal["succeeded"] and terminal["exit_code"] == 0
        and terminal["diagnostics"]["cleanup"]["complete"]
        and receipt["action_key"] == terminal["action_key"] == key
        and terminal["result"]["receipt_sha256"] == receipt["receipt_sha256"]):
    raise ValueError("trial lacks a successful terminal record, cleanup or matching receipt")
records = Path(__file__).resolve().parents[1] / "tests/maker/records"
(records / f"{args.prefix}result.log").write_bytes(payload)
for name, value in (("receipt", receipt), ("terminal", terminal)):
    (records / f"{args.prefix}{name}.json").write_text(json.dumps(value, indent=2) + "\n")
print(f"Refreshed {args.prefix or 'maker '}record from {key}, receipt {receipt['receipt_sha256']}")
