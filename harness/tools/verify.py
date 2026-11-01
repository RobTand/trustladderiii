"""Run only as a trusted outer process inside an admitted PrismaBuild action."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

from tl3_harness.execution import Container, require_admission, result_json, run_checks
from tl3_harness.model import Check, Custody, Kind, Verdict
from tl3_harness.trees import git


def save(repo, message):
    git(repo, "add", ".")
    # PB snapshots give fixture files the same timestamps. Rehash equal-sized
    # edits so Git's index stat cache cannot hide the changed assertion.
    git(repo, "add", "--renormalize", ".")
    git(repo, "-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", "commit", "-qm", message)
    return git(repo, "rev-parse", "HEAD").decode().strip()


def main():
    require_admission()
    container = Container(sys.argv[1])
    source = Path(__file__).resolve().parents[1]
    suite = ["docker", "run", "--rm", "--pull=never", "--network=none", "--read-only",
             "--cap-drop=ALL", "--security-opt=no-new-privileges", "--user=65534:65534",
             "--pids-limit=128", "--tmpfs", "/tmp:rw,nosuid,nodev,size=256m,mode=1777",
             "--mount", f"type=bind,src={source},dst=/source,readonly",
             "--env", "HOME=/tmp", "--env", "TMPDIR=/tmp", "--env", "PYTHONPATH=/source/src",
             "--env", "PYTEST_DISABLE_PLUGIN_AUTOLOAD=1", "--env", "OMP_NUM_THREADS=1",
             "--env", "MKL_NUM_THREADS=1", "--env", "OPENBLAS_NUM_THREADS=1",
             "--workdir=/source", "--entrypoint=python3", container.image,
             "-B", "-m", "pytest", "-q", "-p", "no:cacheprovider", "tests/test_harness.py", "tests/test_review_fixes.py", "tests/test_inventory.py", "tests/test_report_limits.py"]
    subprocess.run(suite, check=True)
    from verify_reproducing import verify
    verify(container)
    from verify_scratch import verify as verify_scratch
    verify_scratch(container)
    fixtures = source / "tests/fixtures"
    with tempfile.TemporaryDirectory(prefix="tl3-fixture-") as directory:
        repo = Path(directory)
        repo.chmod(0o755)
        shutil.copytree(fixtures / "round/base", repo, dirs_exist_ok=True)
        git(repo, "init", "-q")
        (repo / "tests/test_containment.py").write_text(
            "import os, socket\nfrom pathlib import Path\nimport pytest\n"
            "def test_containment():\n"
            f"    assert sorted(os.sched_getaffinity(0)) == {sorted(os.sched_getaffinity(0))!r}\n"
            "    assert not Path('/mnt/shared').exists()\n"
            "    assert not Path('/var/run/docker.sock').exists()\n"
            "    assert not Path('/run/docker.sock').exists()\n"
            "    assert not Path('/home/rob').exists()\n"
            "    assert 'TL3_TEST_SENTINEL' not in os.environ\n"
            "    with pytest.raises(OSError):\n"
            "        socket.create_connection(('1.1.1.1', 443), timeout=1)\n"
            "    with pytest.raises(OSError):\n"
            "        Path('/harness/runner.py').write_text('changed')\n"
            "    with pytest.raises(OSError):\n"
            "        Path('/checkout/product.py').write_text('changed')\n"
        )
        base = save(repo, "base fixture")
        shutil.copytree(fixtures / "round/head", repo, dirs_exist_ok=True)
        head = save(repo, "head fixture")
        assert git(repo, "show", f"{base}:tests/test_product.py") != git(repo, "show", f"{head}:tests/test_product.py")
        assert git(repo, "show", f"{head}:tests/test_product.py") == (repo / "tests/test_product.py").read_bytes()
        cases = [
            (Kind.DEMONSTRATING, "test_demonstrates", Verdict.PASS),
            (Kind.DEMONSTRATING, "test_does_not_demonstrate", Verdict.FAIL),
            (Kind.DEMONSTRATING, "test_setup", Verdict.FAIL),
            (Kind.DEMONSTRATING, "test_runtime_error", Verdict.FAIL),
            (Kind.DEMONSTRATING, "test_new_name", Verdict.PASS),
            (Kind.REGRESSION, "test_regression_passes", Verdict.PASS),
            (Kind.REGRESSION, "test_regression_breaks", Verdict.FAIL),
            (Kind.REGRESSION, "test_pre_existing", Verdict.PASS),
            (Kind.BEHAVIOR_PRESERVING, "test_preserved", Verdict.PASS),
            (Kind.BEHAVIOR_PRESERVING, "test_regression_breaks", Verdict.FAIL),
            (Kind.REGRESSION, "test_skipped", Verdict.FAIL),
            (Kind.REGRESSION, "test_xfailed", Verdict.FAIL),
            (Kind.REGRESSION, "uncollected", Verdict.FAIL),
            (Kind.REGRESSION, "test_missing_name", Verdict.FAIL),
        ]
        plan = tuple(Check(f"{kind}/{name}", kind, (f"tests/test_product.py::{name}",))
                     for kind, name, _ in cases)
        plan += (Check("containment", Kind.BEHAVIOR_PRESERVING,
                       ("tests/test_containment.py::test_containment",)),
                 Check("missing-file", Kind.REGRESSION, ("tests/test_missing.py::test_missing",)))
        os.environ["TL3_TEST_SENTINEL"] = "fixture-only"
        results = run_checks(repo, base, head, plan, container=container)
        print(result_json(results), flush=True)
        expected = [expected for _, _, expected in cases] + [Verdict.PASS, Verdict.FAIL]
        assert [result.verdict for result in results] == expected
        assert all(result.custody == Custody.IN_PROCESS for result in results)
        assert results[7].tests[0].pre_existing
        for fixture, verdict in [("no_report.py", Verdict.INFRASTRUCTURE_FAILURE),
                                 ("collection_error.py", Verdict.FAIL)]:
            shutil.copyfile(fixtures / fixture, repo / "tests/test_product.py")
            broken = save(repo, fixture)
            result = run_checks(repo, base, broken,
                                (Check(fixture, Kind.DEMONSTRATING,
                                       ("tests/test_product.py::test_demonstrates",)),), container=container)
            print(result_json(result), flush=True)
            assert result[0].verdict == verdict
    print(json.dumps({"verification": "passed", "container_image": container.image}), flush=True)


if __name__ == "__main__":
    main()
