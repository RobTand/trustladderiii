"""Qualify generator containment in one admitted action, without recursive submission.

pbrun --container-image IMAGE --progress-phase tests=180 --timeout-s 180 -- \
    python3 -I harness/tools/verify_generator.py IMAGE
"""
import base64
import json
from pathlib import Path
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src"))
from tl3_harness import generator  # noqa: E402
from tl3_harness.trees import git  # noqa: E402


def main():
    image = generator.Container(sys.argv[1])
    with tempfile.TemporaryDirectory(prefix="tl3-generator-qualification-") as directory:
        root = Path(directory)
        (root / "tests").mkdir()
        (root / "tests/test_old.py").write_text("from socket_support import short_socket\n")
        (root / "tests/socket_support.py").write_text("def short_socket(): return 'short'\n")
        (root / "tests/test_new.py").write_text("from socket_support import short_socket\n")
        (root / "tests/BUILD.bazel").write_text("stale\n")
        (root / "generate.py").write_text('''#!/usr/bin/env python3
from pathlib import Path
import subprocess
import sys
# This must succeed as uid 65534, with only the synthetic read-only index.
paths = subprocess.check_output(["git", "ls-files"]).decode().splitlines()
assert "tests/socket_support.py" in paths and "tests/test_new.py" in paths
assert not Path(".git/hooks").exists()
assert subprocess.check_output(["git", "remote"]).strip() == b""
assert not Path("/mnt/shared/prismabuild-fleet").exists()
assert not any(name.startswith("PRISMABUILD_") for name in __import__("os").environ)
output = "\\n".join(paths) + "\\n"
if "--check" in sys.argv:
    raise SystemExit(Path("tests/BUILD.bazel").read_text() != output)
Path("tests/BUILD.bazel").write_text(output)
if "--escape" in sys.argv:
    Path("tests/test_old.py").write_text("escaped\\n")
if "--fail" in sys.argv:
    raise SystemExit(7)
''')
        (root / "generate.py").chmod(0o755)
        contract = {"command": "./generate.py", "generated_files": ["**/BUILD.bazel"],
                    "protected_paths": [], "files": ["tests/BUILD.bazel"]}
        before = generator.manifest(root)
        artifact = generator.run(root, image, **contract)
        assert artifact["status"] == "completed", artifact
        after = generator.validate_artifact(artifact, before, contract)
        assert generator.manifest(root) == before
        for output in artifact["outputs"]:
            (root / output["path"]).write_bytes(base64.b64decode(output["data"], validate=True))
        assert generator.manifest(root) == after
        checked = generator.run(root, image, **{**contract, "command": "./generate.py --check"})
        assert checked["status"] == "completed" and checked["outputs"] == [], checked
        repeated = generator.run(root, image, **contract)
        assert repeated["status"] == "completed" and repeated["outputs"] == [], repeated
        escaped = generator.run(root, image, **{**contract, "command": "./generate.py --escape"})
        assert escaped["status"] == "refused" and "undeclared" in escaped["message"], escaped
        failed = generator.run(root, image, **{**contract, "command": "./generate.py --fail"})
        assert failed["status"] == "failed" and failed["exit_code"] == 7, failed
        assert generator.manifest(root) == after
        # An independent Git tree records exactly the imported bytes.
        git(root, "init", "--quiet", "--template=")
        git(root, "add", "--force", "--all")
        tree = git(root, "write-tree").decode().strip()
        print(json.dumps({"status": "completed", "image": image.image, "output_tree": tree,
                          "input_manifest": generator.digest(before), "output_manifest": generator.digest(after),
                          "artifact_sha256": generator.digest(artifact), "independent_check": checked["status"],
                          "idempotent": True, "undeclared_write": escaped["status"],
                          "failure_exit_code": failed["exit_code"]}))


if __name__ == "__main__":
    main()
