"""The slice's kanban board, kept as GitHub issues.

Each ticket is a sub-issue of the PRD, with GitHub's native "blocked by"
edges between tickets and labels for kind and status. Calls go through the
REST API only, because Rob's GraphQL budget is shared.

    board.py publish TICKETS.json   create the tickets, edges and labels
    board.py show                   print the columns and the frontier
    board.py move NUMBER STATUS     set a ticket's status label

TICKETS.json is a list of {"key", "title", "body", "kind", "blocked_by"},
in dependency order, where kind is "afk" or "hitl" and blocked_by lists keys.
"""
import json
import subprocess
import sys
import time

REPO = "RobTand/trustladderiii"
PARENT = 1
KINDS = {
    "afk": ("0e8a16", "An agent can do it alone"),
    "hitl": ("d93f0b", "Needs Rob for one step"),
}
STATUSES = {
    "ready-for-agent": ("1d76db", "Every blocker is done; an agent can start"),
    "in-progress": ("fbca04", "An agent is working on it"),
    "in-review": ("5319e7", "Its PR is under review"),
}


def api(method, path, body=None):
    cmd = ["gh", "api", "-X", method, f"repos/{REPO}/{path}"]
    if body is not None:
        cmd += ["--input", "-"]
    done = subprocess.run(cmd, input=json.dumps(body) if body is not None else None,
                          capture_output=True, text=True)
    time.sleep(0.5)
    if done.returncode:
        raise SystemExit(f"{method} {path}: {done.stderr.strip() or done.stdout.strip()}")
    return json.loads(done.stdout) if done.stdout.strip() else None


def ensure_labels():
    have = {label["name"] for label in api("GET", "labels?per_page=100")}
    for name, (color, description) in {**KINDS, **STATUSES}.items():
        if name not in have:
            api("POST", "labels", {"name": name, "color": color, "description": description})


def publish(path):
    tickets = json.load(open(path))
    ensure_labels()
    made = {}
    for t in tickets:
        missing = [k for k in t["blocked_by"] if k not in made]
        if missing:
            raise SystemExit(f"{t['key']} is blocked by {missing}, which come later; order the file")
        labels = [t["kind"]]
        if not t["blocked_by"] and t["kind"] == "afk":
            labels.append("ready-for-agent")
        issue = api("POST", "issues", {"title": t["title"], "body": t["body"], "labels": labels})
        made[t["key"]] = issue
        api("POST", f"issues/{PARENT}/sub_issues", {"sub_issue_id": issue["id"]})
        for k in t["blocked_by"]:
            api("POST", f"issues/{issue['number']}/dependencies/blocked_by", {"issue_id": made[k]["id"]})
        print(f"#{issue['number']} {t['key']} {t['title']}")


def tickets():
    rows = []
    for sub in api("GET", f"issues/{PARENT}/sub_issues?per_page=100"):
        blockers = api("GET", f"issues/{sub['number']}/dependencies/blocked_by?per_page=100")
        rows.append({
            "number": sub["number"],
            "title": sub["title"],
            "state": sub["state"],
            "labels": {label["name"] for label in sub["labels"]},
            "open_blockers": [b["number"] for b in blockers if b["state"] == "open"],
        })
    return rows


def column(t):
    if t["state"] == "closed":
        return "done"
    for status in ("in-review", "in-progress"):
        if status in t["labels"]:
            return status
    return "blocked" if t["open_blockers"] else "ready"


def show():
    rows = tickets()
    for name in ("ready", "in-progress", "in-review", "blocked", "done"):
        group = [t for t in rows if column(t) == name]
        print(f"\n{name.upper()} ({len(group)})")
        for t in group:
            kind = "hitl" if "hitl" in t["labels"] else "afk"
            waits = f"  waits on {', '.join(f'#{n}' for n in t['open_blockers'])}" if t["open_blockers"] else ""
            print(f"  #{t['number']:<4} {kind:<4} {t['title']}{waits}")


def move(number, status):
    if status not in STATUSES and status != "none":
        raise SystemExit(f"status is one of {sorted(STATUSES)} or none")
    issue = api("GET", f"issues/{number}")
    labels = [label["name"] for label in issue["labels"] if label["name"] not in STATUSES]
    if status != "none":
        labels.append(status)
    api("PUT", f"issues/{number}/labels", {"labels": labels})


if __name__ == "__main__":
    match sys.argv[1:]:
        case ["publish", path]:
            publish(path)
        case ["show"]:
            show()
        case ["move", number, status]:
            move(int(number), status)
        case _:
            sys.exit(__doc__)
