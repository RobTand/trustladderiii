"""Print the PR #58 review verification campaign, from a clean detached worktree.

Submit the output with pbcampaign.py. This only describes work; every check
executes on an admitted PrismaBuild worker. Keep the manifest outside the repo.
"""
import json
from pathlib import Path
import subprocess
import sys

root = Path(__file__).resolve().parents[3]
rows = json.loads(subprocess.check_output([
    sys.executable, str(root / "crates/kernel/run.py"), "manifest", "tests",
    "apply_ack_retryable", "advance_publishing", "progress_from_queued",
], text=True))
rows.insert(0, {
    "cwd": str(root),
    "argv": ["bash", "crates/controller/tools/verify.sh"],
    "cpus": 2,
    "demand": {"mem_gb": 6},
    "anywhere": True,
    "env": {**rows[0]["env"], "CARGO_BUILD_JOBS": "2",
            "MKL_NUM_THREADS": "1", "OPENBLAS_NUM_THREADS": "1"},
    "priority": -10,
    "timeout_s": 3600,
})
print(json.dumps(rows, indent=2))
