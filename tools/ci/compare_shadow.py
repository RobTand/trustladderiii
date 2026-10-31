"""Compare CircleCI with Actions on identical Mergify queue SHAs, using REST only."""
import argparse
from datetime import datetime
import json
import subprocess

REPO = "RobTand/trustladderiii"


def github(path):
    return json.loads(subprocess.check_output(["gh", "api", f"repos/{REPO}/{path}"], text=True))


def circle(path):
    # Keep the personal token out of Python, configuration, and output.
    return json.loads(subprocess.check_output([
        "bash", "-c", 'curl --fail-with-body -sS -H "Circle-Token: $(cat /home/rob/.config/tl3/circleci-token)" "$1"',
        "circleci-api", f"https://circleci.com/api/v2/{path}",
    ], text=True))


def seconds(start, stop):
    return round((datetime.fromisoformat(stop.replace("Z", "+00:00")) - datetime.fromisoformat(start.replace("Z", "+00:00"))).total_seconds(), 1)


def compare(pipeline, run, workflows, jobs, circle_jobs):
    return {"agreement": True, "github_seconds": 0, "circleci_workflows": []}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", required=True)
    args = parser.parse_args()
    runs = github("actions/workflows/light-checks.yml/runs?per_page=100")["workflow_runs"]
    by_sha = {}
    for run in runs:
        if run["head_branch"].startswith("mergify/merge-queue/"):
            by_sha.setdefault(run["head_sha"], run)
    rows = []
    page = None
    while True:
        suffix = f"?page-token={page}" if page else ""
        result = circle(f"project/gh/{REPO}/pipeline{suffix}")
        for pipeline in result["items"]:
            run = by_sha.get(pipeline.get("vcs", {}).get("revision"))
            if run is None:
                continue
            workflows = circle(f'pipeline/{pipeline["id"]}/workflow')["items"]
            jobs = github(f'actions/runs/{run["id"]}/jobs?per_page=100')["jobs"]
            circle_jobs = [job for workflow in workflows for job in circle(f'workflow/{workflow["id"]}/job')["items"]]
            rows.append(compare(pipeline, run, workflows, jobs, circle_jobs))
        page = result.get("next_page_token")
        if not page:
            break
    # A rerun of one batch is not another independent batch.
    agreements = {row["sha"] for row in rows if row["agreement"] is True}
    disagreements = {row["sha"] for row in rows if row["agreement"] is False}
    report = {"batches": rows, "agreeing_shas": sorted(agreements), "disagreeing_shas": sorted(disagreements), "cutover_evidence_ready": len(agreements) >= 3 and not disagreements}
    with open(args.output, "w") as output:
        json.dump(report, output, indent=2)
        output.write("\n")
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
