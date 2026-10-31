"""Queue a TL3 pull request through Mergify, only when it is safe to.

    queue_pr.py PR_NUMBER EXPECTED_HEAD_SHA

Rob installed Mergify on trustladderiii on 2026-10-30 (#7). TL3's own
`.mergify.yml` is a serial, in-place queue that enqueues only by explicit
command, so this posts `@mergifyio queue` once the PR is ready. Mergify then
updates the branch from main, waits for Rust and Python on the updated head, and
merges. Refuses unless the head matches, Rust and Python passed on it, and the PR
has no conflicts. `merge_pr.py` merges directly and is only for when Mergify is
down. REST only.
"""
import json
import subprocess
import sys

REPO = "RobTand/trustladderiii"
REQUIRED = ("Rust", "Python")


def api(*args):
    done = subprocess.run(["gh", "api", *args], capture_output=True, text=True)
    if done.returncode:
        raise SystemExit(f"gh api {' '.join(args)}: {done.stderr.strip() or done.stdout.strip()}")
    return json.loads(done.stdout) if done.stdout.strip() else None


def main(number, expected):
    pr = api(f"repos/{REPO}/pulls/{number}")
    head = pr["head"]["sha"]
    if not head.startswith(expected):
        raise SystemExit(f"#{number} head is {head}, not {expected}")
    if pr["state"] != "open" or pr["draft"]:
        raise SystemExit(f"#{number} is {pr['state']}{' (draft)' if pr['draft'] else ''}")
    runs = api(f"repos/{REPO}/commits/{head}/check-runs")["check_runs"]
    for name in REQUIRED:
        found = [r for r in runs if r["name"] == name and r["app"]["slug"] == "github-actions"]
        if not found:
            raise SystemExit(f"#{number}: no {name} check on {head[:12]}")
        latest = max(found, key=lambda r: r["started_at"] or "")
        if latest["status"] != "completed" or latest["conclusion"] != "success":
            raise SystemExit(f"#{number}: {name} is {latest['status']}/{latest['conclusion']} on {head[:12]}")
    if pr["mergeable"] is False or pr["mergeable_state"] == "dirty":
        raise SystemExit(f"#{number} has conflicts with main; merge main into it first")
    comment = api(f"repos/{REPO}/issues/{number}/comments", "-f", "body=@mergifyio queue")
    print(f"#{number} queued through Mergify at {head[:12]}: {comment['html_url']}")


if __name__ == "__main__":
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    main(sys.argv[1], sys.argv[2])
