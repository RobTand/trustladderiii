"""Run once from the submitting session, never inside a PrismaBuild action."""
import argparse
import json
import os
from pathlib import Path
import tempfile

from tl3_workers.pb_tests import TestSettings, make_test_tool
from tl3_workers.tools import Worktree


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("image")
    parser.add_argument("--tools-path", type=Path, default=Path("/mnt/shared/prismabuild-fleet/repo/tools"))
    args = parser.parse_args()
    if os.environ.get("PRISMABUILD_ACTION_PROGRESS_TOKEN"):
        raise RuntimeError("the trial must submit from outside an admitted action")
    with tempfile.TemporaryDirectory(prefix="tl3-test-trial-") as directory:
        root = Path(directory)
        (root / "test_trial.py").write_text(
            'import os\nimport socket\nimport pytest\nfrom pathlib import Path\n'
            'def test_containment():\n'
            '    assert not Path("/mnt/shared/prismabuild-fleet").exists()\n'
            '    assert not Path("/var/run/docker.sock").exists()\n'
            '    assert {name for _, name in socket.if_nameindex()} == {"lo"}\n'
            '    with pytest.raises(OSError):\n'
            '        Path("/harness/write-probe").write_text("forbidden")\n'
            '    assert "PRISMABUILD_ACTION_PROGRESS_TOKEN" not in os.environ\n'
            '    with pytest.raises(OSError):\n'
            '        Path("/checkout/write-probe").write_text("forbidden")\n'
            'def test_failure():\n'
            '    assert False, "trial assertion"\n'
            '@pytest.mark.skip(reason="trial skip")\n'
            'def test_skipped():\n'
            '    pass\n'
        )
        with Worktree(root) as tree:
            result = make_test_tool(tree, TestSettings(args.image, args.tools_path))()
        print(json.dumps(result, sort_keys=True), flush=True)
        if result["status"] != "completed" or result["totals"] != {
            "passed": 1, "failed": 1, "error": 0, "skipped": 1,
        }:
            raise SystemExit(1)


if __name__ == "__main__":
    main()
