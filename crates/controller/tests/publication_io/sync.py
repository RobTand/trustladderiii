"""Refresh E1 shell copies. The remote Rust test checks every copied body and pin."""
from pathlib import Path
import re
import subprocess

ROOT = Path(__file__).resolve().parents[4]
HERE = Path(__file__).resolve().parent
BASE = "cdacce8f99c28ca8dba8f5f76831b77b4dfcd836"
GITHUB = "crates/controller/src/adapters/github.rs"
PUBLISHING = "crates/controller/src/adapters/github/publishing.rs"
RECORDS = "crates/controller/src/adapters/record.rs"
METHODS = ["body_lines", "semantic_paths", "fence", "client", "request_notice", "linked",
           "file_request", "console_note", "restart", "close_reason", "execute_delivery", "reconcile", "repair_approval", "reconcile_cancellation", "publication_comment", "publication_route"]

def function(source, name, indent=0, after=""):
    offset = source.index(after) if after else 0
    pattern = rf"(?m)^{' ' * indent}(?:pub(?:\([^)]*\))? )?fn {name}(?:\(|<)"
    match = re.search(pattern, source[offset:])
    start = offset + match.start()
    end = source.index("\n" + " " * indent + "}\n", start) + indent + 3
    return source[start:end]


def render(source):
    bodies = []
    for name in METHODS:
        if name in ("publication_comment", "publication_route") and f"fn {name}(" not in source:
            continue
        after = "impl Executor for Adapter {" if name in ("reconcile", "repair_approval", "reconcile_cancellation") else ""
        bodies.append(function(source, name, 4, after))
    return "use super::*;\nimpl Adapter {\n" + "".join(bodies) + "}\n"


def async_function(source, name):
    start = re.search(rf"(?m)^    (?:pub )?async fn {name}\(", source).start()
    end = source.index("\n    }\n", start) + 7
    return source[start:end]


def main():
    for path, stem in [(GITHUB, "github"), (PUBLISHING, "publishing"), (RECORDS, "records")]:
        base = subprocess.check_output(["git", "show", f"{BASE}:{path}"], cwd=ROOT).decode()
        frozen = HERE / f"base-{stem}.rs.txt"
        if not frozen.exists():
            frozen.write_text(base)
        elif frozen.read_text() != base:
            raise SystemExit(f"frozen source changed: {frozen}")
        if stem == "github":
            (HERE / "reference.rs").write_text(render(base))
            (HERE / "current.rs").write_text(render((ROOT / path).read_text()))
        elif stem == "publishing":
            (HERE / "reference-publishing.rs").write_text(base)
            (HERE / "current-publishing.rs").write_text((ROOT / path).read_text())
        else:
            (HERE / "records.rs").write_text(base[:base.index("\npub fn delivery_counts")])
    github = (ROOT / GITHUB).read_text()
    (HERE / "message.rs").write_text(function(github, "question_draft")
                                     + function(github, "fixed_notice_text"))
    evidence = (ROOT / "crates/controller/src/evidence.rs").read_text()
    (HERE / "evidence.rs").write_text("use super::*;\npub use tl3_controller::evidence::{publication, with_publication};\nconst BODY_BYTES: usize = 60_000;\n" + "".join(function(evidence, n) for n in ["publication_goals", "required_publication_lines", "publication_lines", "validate_publication", "validate_body"]))
    policy = (ROOT / "crates/controller/src/pr_body.rs").read_text()
    names = re.findall(r"(?m)^(?:pub(?:\([^)]*\))? )?fn (\w+)", policy)
    # All top-level production functions, including compose after the test module.
    (HERE / "pr_body.rs").write_text("use crate::publication_failure::{LocalRefusal, Reason, RuleCategory, RuleFailure};\nuse tl3_config::BodyLine;\n" + "".join(function(policy, n) for n in names))
    goals = (ROOT / "crates/controller/src/goals.rs").read_text()
    (HERE / "goals.rs").write_text(function(goals, "branch_satisfies"))
    client = (ROOT / "crates/github/src/client.rs").read_text()
    (HERE / "base-client.rs.txt").write_text(subprocess.check_output(["git", "show", f"{BASE}:crates/github/src/client.rs"], cwd=ROOT).decode())
    (HERE / "comments.rs").write_text("use reqwest::{Method, Url};\nuse tl3_github::Error;\ntype Result<T,E=Error> = ::std::result::Result<T,E>;\nimpl GitHub {\n" + function(client,"owns_comment",4) + "".join(async_function(client,n) for n in ["post_comment", "post_formatted_comment", "reconcile_comments", "delete_comment"]) + "}\n" + "".join(function(client,n) for n in ["comment_marker","marked_body","repo_path","segment"]))
    failures = (ROOT / "crates/controller/src/publication_failure.rs").read_text()
    end = failures.index("impl Diagnostic {")
    (HERE / "failure.rs").write_text(failures[:end] + "impl Diagnostic {\n" + function(failures, "render", 4) + "}\n")
    worker = (ROOT / "crates/controller/src/worker_inputs.rs").read_text()
    start = worker.index("#[derive(Clone, Debug, Deserialize, Serialize)]\npub(crate) struct Context")
    end = worker.index("\nimpl Context", start)
    display = worker[worker.index("impl std::fmt::Display for Context"):worker.index("\nfn stored(")]
    (HERE / "worker_inputs.rs").write_text("use serde::{Deserialize,Serialize};\n" + worker[worker.index("#[derive(Clone, Debug, Deserialize, Serialize, thiserror::Error)]"):worker.index("\nimpl Failure")] + worker[start:end] + "\nimpl Context {\n" + function(worker,"description",4) + "}\n" + display)

if __name__ == "__main__":
    main()
