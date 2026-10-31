"""Queue a reviewed TL3 head through Mergify.

    queue_pr.py PR_NUMBER EXPECTED_HEAD_SHA

The coordinator reviews the full expected SHA and PrismaBuild evidence before
calling this. PR-level CI is optional. Mergify tests a batch and fast-forwards
main to its tested head after Rust and Python pass. REST only.
"""
import json
import subprocess
import sys

REPO = "RobTand/trustladderiii"


def api(*args):
    done = subprocess.run(["gh", "api", *args], capture_output=True, text=True)
    if done.returncode:
        raise SystemExit(f"gh api {' '.join(args)}: {done.stderr.strip() or done.stdout.strip()}")
    return json.loads(done.stdout) if done.stdout.strip() else None


def main(number, expected):
    pr = api(f"repos/{REPO}/pulls/{number}")
    head = pr["head"]["sha"]
    if len(expected) != 40 or head != expected:
        raise SystemExit(f"#{number} head is {head}, not {expected}")
    if pr["state"] != "open" or pr["draft"] or pr["merged"]:
        raise SystemExit(f"#{number} is {pr['state']}{' (draft)' if pr['draft'] else ''}")
    if pr["base"]["ref"] != "main" or pr["base"]["repo"]["full_name"] != REPO:
        raise SystemExit(f"#{number} must target {REPO}'s main branch")
    if pr["mergeable"] is False or pr["mergeable_state"] == "dirty":
        raise SystemExit(f"#{number} has conflicts with main; merge main into it first")
    if pr["mergeable"] is not True:
        raise SystemExit(f"#{number} mergeability is unknown; retry after GitHub computes it")
    comment = api(f"repos/{REPO}/issues/{number}/comments", "-f", "body=@mergifyio queue")
    print(f"#{number} queued through Mergify at {head[:12]}: {comment['html_url']}")


if __name__ == "__main__":
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    main(sys.argv[1], sys.argv[2])
