import importlib.util
import ast
import io
import contextlib
import hashlib
import json
import os
import re
from pathlib import Path
import socket
import stat
import subprocess
import tempfile
import time
import tomllib
import unittest
from types import SimpleNamespace
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("deployment", Path(__file__).with_name("deploy.py"))
deploy = importlib.util.module_from_spec(spec)
spec.loader.exec_module(deploy)


def postgres_references(python, rust, units):
    """Audit paths derived from paths(home)'s PG tree, plus Rust/units commands."""
    references = set()
    for function in ast.walk(ast.parse(python)):
        if not isinstance(function, ast.FunctionDef):
            continue
        pg_roots = set()
        for node in ast.walk(function):
            if (isinstance(node, ast.Assign) and isinstance(node.value, ast.Call)
                    and isinstance(node.value.func, ast.Name) and node.value.func.id == "paths"):
                for target in node.targets:
                    if isinstance(target, ast.Tuple):
                        pg_roots.add(target.elts[2].id)
        for node in ast.walk(function):
            if isinstance(node, ast.Call) and isinstance(node.func, ast.Name) and node.func.id == "pg_executable":
                if not isinstance(node.args[1], ast.Constant):
                    if function.name != "verify_postgres_executables":
                        raise AssertionError("dynamic PG executable outside the declared-set verifier")
                else:
                    references.add(node.args[1].value)
            if isinstance(node, ast.BinOp) and isinstance(node.op, ast.Div):
                left = node.left
                while isinstance(left, ast.BinOp):
                    left = left.left
                if isinstance(left, ast.Name) and left.id in pg_roots:
                    raise AssertionError("PG tree paths must use pg_executable")
            if (isinstance(node, ast.Call) and isinstance(node.func, ast.Attribute)
                    and isinstance(node.func.value, ast.Name) and node.func.value.id in pg_roots
                    and node.func.attr in ("joinpath",)):
                raise AssertionError("PG tree paths must use pg_executable")
    for root, executable in re.findall(r'Command::new\(([^()]+)\.join\("([^\"]+)"\)\)', rust):
        # These are the only PGtree roots in helper.rs's disposable-cluster tests.
        if root in ("binary", "self.0"):
            references.add(executable)
    references.update(re.findall(r'postgresql-[^/\s]+/bin/([^\s]+)', units))
    return references


class PostgresPackageTests(unittest.TestCase):
    def test_pg_tree_executables_are_declared(self):
        python = (deploy.HERE / "deploy.py").read_text()
        rust = (deploy.HERE / "helper.rs").read_text()
        units = "\n".join(path.read_text() for path in (deploy.HERE / "units").glob("*.service"))
        references = postgres_references(python, rust, units)
        self.assertLessEqual(references, set(deploy.PG_EXECUTABLES))
        self.assertIn("initdb", references)
        self.assertIn("pg_ctl", references)
        for utility in ("pg_controldata", "pg_isready", "psql", "vacuumdb"):
            mutant = python.replace('data = root / "postgres/data"',
                                    f'data = binaries / "bin/{utility}"', 1)
            with self.assertRaisesRegex(AssertionError, "PG tree paths"):
                postgres_references(mutant, rust, units)
            with self.assertRaisesRegex(RuntimeError, "outside"):
                deploy.pg_executable(Path("/synthetic/pg"), utility)

    def test_install_checks_entire_declared_set_before_publishing_and_on_reuse(self):
        for missing in deploy.PG_EXECUTABLES:
            with self.subTest(missing=missing), tempfile.TemporaryDirectory() as temporary:
                home = Path(temporary)
                _, _, binaries = deploy.paths(home)
                binaries.mkdir(parents=True)
                for name in deploy.PG_EXECUTABLES:
                    if name != missing:
                        deploy.write_file(deploy.pg_executable(binaries, name), b"#!/bin/sh\nexit 0\n", 0o700)
                with self.assertRaisesRegex(RuntimeError, "incomplete"):
                    deploy.postgres(home)
                self.assertFalse((deploy.paths(home)[1] / "secrets").exists())
        with tempfile.TemporaryDirectory() as temporary:
            home = Path(temporary)
            _, _, binaries = deploy.paths(home)
            def extract(tar, destination, **kwargs):
                for name in deploy.PG_EXECUTABLES[:-1]:
                    deploy.write_file(deploy.pg_executable(destination, name), b"#!/bin/sh\n", 0o700)
            txz = io.BytesIO()
            with deploy.tarfile.open(fileobj=txz, mode="w:xz"):
                pass
            jar = io.BytesIO()
            with deploy.zipfile.ZipFile(jar, "w") as archive:
                archive.writestr("postgres.txz", txz.getvalue())
            def download(url, digest, destination):
                destination.write_bytes(jar.getvalue())
            with patch.object(deploy, "checked_download", side_effect=download), \
                 patch.object(deploy.tarfile.TarFile, "extractall", extract):
                with self.assertRaisesRegex(RuntimeError, "incomplete"):
                    deploy.postgres(home)
            self.assertFalse(binaries.exists())

    def test_readiness_retries_refusal_and_timeout_with_bounded_processes(self):
        for failure in ("connection refused", "tl3-deploy timed out; output suppressed"):
            with self.subTest(failure=failure), \
                 patch.object(deploy, "run", side_effect=[RuntimeError(failure), b""]) as run, \
                 patch.object(deploy.time, "sleep") as sleep:
                deploy.wait_for_postgres(Path("/selected/tl3-deploy"), Path("/synthetic/config"))
                self.assertEqual(run.call_count, 2)
                self.assertTrue(all(call.kwargs["timeout"] == 4 for call in run.call_args_list))
                sleep.assert_called_once_with(0.5)
        with patch.object(deploy, "run", side_effect=RuntimeError("refused")) as run, \
             patch.object(deploy.time, "sleep") as sleep:
            with self.assertRaisesRegex(RuntimeError, "did not become ready"):
                deploy.wait_for_postgres(Path("/selected/tl3-deploy"), Path("/synthetic/config"))
            self.assertEqual(run.call_count, 3)
            self.assertEqual(sleep.call_count, 2)


RELEASE_TARGETS = [("lib", "tl3_controller"), ("bin", "tl3"), ("bin", "tl3d"), ("bin", "tl3-run-backend"),
                   ("bin", "tl3-worker-supervisor"), ("example", "tl3-deploy")]


def release_messages(features=(), targets=RELEASE_TARGETS):
    """Cargo --message-format=json compiler-artifact lines for the tl3-controller release build."""
    package = "path+file:///fixture/crates/controller#tl3-controller@0.1.0"
    lines = [json.dumps({"reason": "compiler-artifact", "package_id": package,
                         "target": {"kind": [kind], "name": name}, "features": list(features)})
             for kind, name in targets]
    lines.append(json.dumps({"reason": "build-finished", "success": True}))
    return "\n".join(lines) + "\n"


def fake_release_cargo(path, messages, extra=""):
    """A fake cargo that writes release binaries and prints the given JSON messages on stdout."""
    path.write_text("#!/bin/sh\nmkdir -p \"$CARGO_TARGET_DIR/release/examples\"\n"
                    "for name in tl3 tl3d tl3-run-backend tl3-worker-supervisor; do printf binary > \"$CARGO_TARGET_DIR/release/$name\"; done\n"
                    + extra + "cat <<'CARGO_MESSAGES'\n" + messages + "CARGO_MESSAGES\n")
    path.chmod(0o700)


class ReleasePublicationTests(unittest.TestCase):
    def test_failed_copy_does_not_publish_or_wedge_release_retry(self):
        script = Path(__file__).with_name("build-release.sh").resolve()
        with tempfile.TemporaryDirectory(prefix="tl3-build-publication-") as temporary:
            root = Path(temporary)
            tools = root / "tools"
            tools.mkdir()
            git = tools / "git"
            git.write_text("#!/bin/sh\ncase \"$*\" in *status*) exit 0;; *rev-parse*) echo aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa;; esac\n")
            fake_release_cargo(tools / "cargo", release_messages(),
                               "if [ \"$TL3_TEST_SKIP_HELPER\" = 0 ]; then printf helper > \"$CARGO_TARGET_DIR/release/examples/tl3-deploy\"; fi\n")
            git.chmod(0o700)
            output = root / "release"
            environment = os.environ | {"PATH": str(tools) + os.pathsep + os.environ["PATH"],
                                         "CARGO_TARGET_DIR": str(root / "target"), "TL3_TEST_SKIP_HELPER": "1"}
            first = subprocess.run(["bash", str(script), str(output), "a" * 40], env=environment,
                                   capture_output=True, timeout=30)
            self.assertNotEqual(first.returncode, 0)
            self.assertFalse(output.exists(), first.stderr.decode())
            environment["TL3_TEST_SKIP_HELPER"] = "0"
            second = subprocess.run(["bash", str(script), str(output), "a" * 40], env=environment,
                                    capture_output=True, timeout=30)
            self.assertEqual(second.returncode, 0, second.stderr.decode())
            self.assertEqual(json.loads((output / "SOURCE.json").read_text())["head"], "a" * 40)

    def test_canary_qualification_staging_bytes_refuse_release(self):
        """FX-REQUAL-43 OQ-1: the example-only staging gate never reaches release artifacts."""
        script = Path(__file__).with_name("build-release.sh").resolve()
        for name, payload in [("tl3d", "tl3.fx_canary_qualification_staging.v1"),
                              ("tl3-run-backend", "tl3.fx_canary_qualification_staging.v1"),
                              ("examples/tl3-deploy", "_ZN4core24for_canary_qualification17h0E")]:
            with self.subTest(binary=name), \
                 tempfile.TemporaryDirectory(prefix="tl3-build-qualification-") as temporary:
                root = Path(temporary)
                tools = root / "tools"
                tools.mkdir()
                git = tools / "git"
                git.write_text("#!/bin/sh\ncase \"$*\" in *status*) exit 0;; *rev-parse*) echo aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa;; esac\n")
                fake_release_cargo(tools / "cargo", release_messages(),
                                   "printf helper > \"$CARGO_TARGET_DIR/release/examples/tl3-deploy\"\n"
                                   "printf '\\000%s\\000' \"$TL3_TEST_PAYLOAD\" > \"$CARGO_TARGET_DIR/release/$TL3_TEST_BINARY\"\n")
                git.chmod(0o700)
                output = root / "release"
                environment = os.environ | {"PATH": str(tools) + os.pathsep + os.environ["PATH"],
                                             "CARGO_TARGET_DIR": str(root / "target"),
                                             "TL3_TEST_BINARY": name, "TL3_TEST_PAYLOAD": payload}
                result = subprocess.run(["bash", str(script), str(output), "a" * 40], env=environment,
                                        capture_output=True, timeout=30)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn(b"refusing release", result.stderr)
                self.assertFalse(output.exists())

    def test_release_build_with_canary_feature_or_unproven_features_refuses(self):
        """FX-REQUAL-43 OQ-1 SHOULD-1: Cargo's own artifact report must show the feature off."""
        script = Path(__file__).with_name("build-release.sh").resolve()
        cases = {
            "feature on the controller library": (
                release_messages(), release_messages(features=["activation-qualification"],
                                                     targets=RELEASE_TARGETS[:1]),
                b"activation-qualification"),
            "feature on a shipped binary": (
                release_messages(), release_messages(features=["activation-qualification"],
                                                     targets=[("bin", "tl3d")]),
                b"activation-qualification"),
            "controller library artifact missing": (
                release_messages(targets=RELEASE_TARGETS[1:]), "", b"no Cargo artifact report"),
            "no artifact report at all": ("", "", b"no Cargo artifact report"),
        }
        for case, (messages, extra_messages, reason) in cases.items():
            with self.subTest(case=case), \
                 tempfile.TemporaryDirectory(prefix="tl3-build-features-") as temporary:
                root = Path(temporary)
                tools = root / "tools"
                tools.mkdir()
                git = tools / "git"
                git.write_text("#!/bin/sh\ncase \"$*\" in *status*) exit 0;; *rev-parse*) echo aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa;; esac\n")
                git.chmod(0o700)
                fake_release_cargo(tools / "cargo", messages + extra_messages,
                                   "printf helper > \"$CARGO_TARGET_DIR/release/examples/tl3-deploy\"\n")
                output = root / "release"
                environment = os.environ | {"PATH": str(tools) + os.pathsep + os.environ["PATH"],
                                             "CARGO_TARGET_DIR": str(root / "target")}
                result = subprocess.run(["bash", str(script), str(output), "a" * 40], env=environment,
                                        capture_output=True, timeout=30)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn(b"refusing release", result.stderr)
                self.assertIn(reason, result.stderr)
                self.assertFalse(output.exists())
                self.assertEqual(sorted(path.name for path in root.iterdir()), ["target", "tools"])

    def test_release_build_asks_cargo_for_its_artifact_report(self):
        text = Path(__file__).with_name("build-release.sh").read_text()
        builds = [line for line in text.splitlines() if line.startswith("cargo build")]
        self.assertEqual(len(builds), 1)
        self.assertIn("--message-format=json-render-diagnostics", builds[0])
        self.assertNotIn("--features", builds[0])


class FreshInitTests(unittest.TestCase):
    def setUp(self):
        self.scratch = tempfile.TemporaryDirectory(prefix="tl3-fresh-test-")
        self.addCleanup(self.scratch.cleanup)
        self.home = Path(self.scratch.name)
        self.root, self.config, _ = deploy.paths(self.home)
        self.cluster = self.root / "postgres/data"
        for directory in (self.cluster / "pg_wal", self.cluster / "pg_tblspc",
                          self.root / "world", self.root / "journal", self.config):
            directory.mkdir(parents=True, mode=0o700, exist_ok=True)
        (self.cluster / "PG_VERSION").write_text("17\n")
        (self.cluster / "pg_wal/segment").write_bytes(b"diagnostic WAL")
        (self.root / "world/snapshot").write_bytes(b"old world")
        (self.root / "journal/receipt").write_bytes(b"old receipt")
        deploy.write_file(self.config / "tl3.toml", b"fixture config\n")
        deploy.write_file(self.config / "production-ready", b"")
        self.sources = {"data": self.cluster, "world": self.root / "world",
                        "journal": self.root / "journal", "config": self.config}
        for path in self.sources.values():
            path.chmod(0o700)
            for child in path.rglob("*"):
                child.chmod(0o700 if child.is_dir() else 0o600)
        self.backup = self.home / "backups/cold"
        self.states = {"tl3d.service": "inactive", "tl3-postgres.service": "inactive"}
        self.calls = []
        self.transport = patch.object(deploy, "run", side_effect=self.transport_run)
        self.transport.start()
        self.addCleanup(self.transport.stop)

    def transport_run(self, command, **kwargs):
        argv = list(map(str, command))
        self.calls.append(argv)
        if "cold-cluster-identity" in argv:
            return b"12345\n"
        if argv[:3] == ["systemctl", "--user", "show"]:
            if "--property=NRestarts" in argv:
                return b"0\n"
            return (self.states[argv[3]] + "\n").encode()
        return b""

    def test_cold_backup_is_complete_and_detects_missing_wal_before_reset(self):
        deploy.cold_backup(self.sources, self.backup)
        self.assertEqual((self.backup / "data/pg_wal/segment").read_bytes(), b"diagnostic WAL")
        deploy.verify_cold_backup(self.sources, self.backup)
        (self.backup / "data/pg_wal/segment").unlink()
        with self.assertRaisesRegex(RuntimeError, "backup"):
            deploy.verify_cold_backup(self.sources, self.backup)
        self.assertEqual((self.root / "world/snapshot").read_bytes(), b"old world")

    def test_backup_and_reset_refuse_while_controller_runs(self):
        self.states["tl3d.service"] = "active"
        with self.assertRaisesRegex(RuntimeError, "stopped"):
            deploy.cold_backup(self.sources, self.backup)
        self.assertFalse(self.backup.exists())
        self.states["tl3d.service"] = "inactive"
        deploy.cold_backup(self.sources, self.backup)
        self.states["tl3d.service"] = "active"
        with self.assertRaisesRegex(RuntimeError, "stopped"):
            deploy.reset_file_state(self.sources, self.backup, ("world", "journal"))
        self.assertTrue((self.root / "world/snapshot").exists())

    def test_reset_refuses_missing_or_mismatched_backup(self):
        with self.assertRaisesRegex(RuntimeError, "backup"):
            deploy.reset_file_state(self.sources, self.backup, ("world", "journal"))
        deploy.cold_backup(self.sources, self.backup)
        (self.root / "journal/receipt").write_bytes(b"changed since backup")
        with self.assertRaisesRegex(RuntimeError, "backup"):
            deploy.reset_file_state(self.sources, self.backup, ("world", "journal"))
        self.assertTrue((self.root / "world/snapshot").exists())

    def test_reset_erases_only_world_and_journal_and_retains_evidence(self):
        unrelated = self.root / "mlflow"
        unrelated.write_bytes(b"unrelated database sentinel")
        deploy.cold_backup(self.sources, self.backup)
        deploy.reset_file_state(self.sources, self.backup, ("world", "journal"))
        self.assertEqual(list((self.root / "world").iterdir()), [])
        self.assertEqual(list((self.root / "journal").iterdir()), [])
        self.assertEqual(unrelated.read_bytes(), b"unrelated database sentinel")
        self.assertTrue((self.cluster / "pg_wal/segment").exists())
        self.assertEqual((self.backup / "world/snapshot").read_bytes(), b"old world")

    def test_backup_refuses_symlink_escape_and_unknown_unit_state(self):
        (self.cluster / "pg_wal/escape").symlink_to(self.home)
        with self.assertRaisesRegex(RuntimeError, "symlink"):
            deploy.cold_backup(self.sources, self.backup)
        (self.cluster / "pg_wal/escape").unlink()
        self.states["tl3-postgres.service"] = "failed"
        with self.assertRaisesRegex(RuntimeError, "stopped"):
            deploy.cold_backup(self.sources, self.backup)

    def test_declared_external_tablespace_is_copied_and_rebound_inside_backup(self):
        target = self.home / "external-tablespace"
        target.mkdir(mode=0o700)
        (target / "page").write_bytes(b"tablespace data")
        (target / "page").chmod(0o600)
        (self.cluster / "pg_tblspc/1234").symlink_to(target)
        self.sources["tablespace-1234"] = target
        deploy.cold_backup(self.sources, self.backup)
        self.assertEqual((self.backup / "data/pg_tblspc/1234/page").read_bytes(), b"tablespace data")
        self.assertEqual((self.backup / "data/pg_tblspc/1234").resolve(), self.backup / "tablespace-1234")
        (target / "page").write_bytes(b"changed")
        with self.assertRaisesRegex(RuntimeError, "backup"):
            deploy.verify_cold_backup(self.sources, self.backup)

    def test_selected_daemon_is_stopped_before_postgres_and_dependents_start_last(self):
        args = SimpleNamespace(home=self.home, head="a" * 40, lane="release", evidence=self.home,
                               binaries=self.home / "release", backup=self.backup,
                               state_manifest=self.home / "scope.json")
        args.binaries.mkdir(mode=0o700)
        for name in deploy.RELEASE_BINARIES:
            (args.binaries / name).write_bytes(b"selected binary")
        hashes = {name: hashlib.sha256(b"selected binary").hexdigest() for name in deploy.RELEASE_BINARIES}
        deploy.write_file(args.binaries / "SOURCE.json", json.dumps({"head": args.head, "binaries": hashes}).encode())
        deploy.write_file(args.binaries / "SHA256SUMS", ''.join(
            f"{hashes[name]}  {name}\n" for name in deploy.RELEASE_BINARIES).encode())
        deploy.write_file(args.state_manifest, b"fixture scope")
        deploy.write_file(self.config / "tl3.toml", b'[github]\nwebhook_listen="127.0.0.1:8742"\n')
        self.states.update({unit: "active" for unit in
                            ("tl3d.service", "tl3-postgres.service", "tl3-mlflow.service",
                             "tl3-status.service", "tl3-console.service")})
        original = self.transport_run

        def transport(command, **kwargs):
            argv = list(map(str, command))
            if argv[:3] == ["systemctl", "--user", "stop"]:
                self.states[argv[3]] = "inactive"
            if argv[:3] == ["systemctl", "--user", "start"]:
                self.states[argv[3]] = "active"
            if "fresh-database" in argv:
                self.assertEqual(self.states["tl3d.service"], "inactive")
                for unit in ("tl3-mlflow.service", "tl3-status.service", "tl3-console.service"):
                    self.assertEqual(self.states[unit], "inactive")
                self.assertEqual((self.backup / "data/pg_wal/segment").read_bytes(), b"diagnostic WAL")
            return original(command, **kwargs)

        with patch.object(deploy, "fresh_scope", return_value=(self.sources, ("world", "journal"), self.config / "tl3.toml")), \
             patch.object(deploy, "run", side_effect=transport), \
             patch.object(deploy, "listener_ready", return_value=True), \
             patch.object(deploy, "cold_cluster_identity", return_value="123456"), patch.object(deploy.time, "sleep"):
            deploy.fresh_init(args)
        stops = [call[-1] for call in self.calls if call[:3] == ["systemctl", "--user", "stop"]]
        self.assertEqual(stops, ["tl3d.service", "tl3-mlflow.service", "tl3-status.service",
                                 "tl3-console.service", "tl3-postgres.service"])
        starts = [call[-1] for call in self.calls if call[:3] == ["systemctl", "--user", "start"]]
        self.assertEqual(starts, ["tl3-postgres.service", "tl3d.service", "tl3-mlflow.service",
                                  "tl3-status.service", "tl3-console.service"])
        reset_index = next(i for i, call in enumerate(self.calls) if "fresh-database" in call)
        controller_index = self.calls.index(["systemctl", "--user", "start", "tl3d.service"])
        self.assertLess(reset_index, controller_index)

    def test_qualification_refusal_does_not_stop_any_unit(self):
        args = SimpleNamespace(home=self.home, head="a" * 40, lane="release", evidence=self.home)
        with patch.object(deploy, "run", side_effect=RuntimeError("qualification refused")):
            with self.assertRaisesRegex(RuntimeError, "qualification"):
                deploy.fresh_init(args)
        self.assertEqual(self.calls, [])

    def test_qualification_requires_authenticated_full_suite_and_exact_head_nightly(self):
        args = SimpleNamespace(head="a" * 40, lane="release")
        gate = SimpleNamespace(Refuse=RuntimeError, BuildBuddy=lambda: object(),
                               postmerge_ok=lambda head: False,
                               commit_inventory=lambda *args: None,
                               fingerprint=lambda *args: "matching-inputs",
                               nightly_proof=lambda *args: (42, "a" * 40))
        with self.assertRaises(RuntimeError):
            deploy.check_fresh_qualification(gate, args)
        gate.postmerge_ok = lambda head: True
        gate.nightly_proof = lambda *args: (42, "b" * 40)
        with self.assertRaises(RuntimeError):
            deploy.check_fresh_qualification(gate, args)
        gate.nightly_proof = lambda *args: None
        with self.assertRaises(RuntimeError):
            deploy.check_fresh_qualification(gate, args)
        gate.nightly_proof = lambda *args: (42, "a" * 40)
        deploy.check_fresh_qualification(gate, args)

    def test_dev_qualification_reads_exact_head_evidence_and_refuses_missing_coverage(self):
        args = SimpleNamespace(head="a" * 40, lane="dev", evidence=self.home)
        seen = []

        def evidence(repo, bb, head, path):
            seen.append((head, path.name))
            return "base", {"tests": []}

        gate = SimpleNamespace(Refuse=RuntimeError, BuildBuddy=lambda: object(), verify_evidence=evidence,
                               verus_and_kernel_tests=lambda *args: "missing proofs",
                               fingerprint=lambda *unused: "inputs", nightly_proof=lambda *unused: (42, args.head))
        with self.assertRaises(RuntimeError):
            deploy.check_fresh_qualification(gate, args)
        self.assertEqual(seen, [(args.head, args.head + ".json")])
        gate.verus_and_kernel_tests = lambda *args: None
        deploy.check_fresh_qualification(gate, args)

    def test_existing_config_is_never_upgraded_or_rebound(self):
        destination = self.config / "tl3.toml"
        original = deploy.render_config(self.home, "192.168.1.107:8033", 987, 123,
                                        "sha256:" + "a" * 64)
        deploy.write_file(destination, original.encode())
        deploy.configure(self.home, "192.168.1.107:8033", {})
        self.assertEqual(destination.read_text(), original)
        incomplete = original.replace('bank = "tl3"\n', '')
        deploy.write_file(destination, incomplete.encode())
        with self.assertRaisesRegex(RuntimeError, "current deployment inputs"):
            deploy.configure(self.home, "192.168.1.107:8033", {})
        self.assertEqual(destination.read_text(), incomplete)

    def scope_fixture(self):
        args = SimpleNamespace(home=self.home, head="a" * 40, binaries=self.home / "selected",
                               state_manifest=self.home / "scope.json", backup=self.backup)
        args.binaries.mkdir(mode=0o700)
        for name in deploy.RELEASE_BINARIES:
            (args.binaries / name).write_bytes(b"selected")
            (args.binaries / name).chmod(0o700)
        (args.binaries / "SOURCE.json").write_text(json.dumps({"head": args.head, "binaries": {
            name: hashlib.sha256(b"selected").hexdigest() for name in deploy.RELEASE_BINARIES}}))
        (args.binaries / "SOURCE.json").chmod(0o600)
        deploy.write_file(args.binaries / "SHA256SUMS", ''.join(
            hashlib.sha256(b"selected").hexdigest() + "  " + name + "\n"
            for name in deploy.RELEASE_BINARIES).encode())
        runs = self.root / "worker-runs"
        journal = runs / ".controller/direct-merge"
        journal.mkdir(parents=True, mode=0o700)
        runs.chmod(0o700)
        journal.parent.chmod(0o700)
        (journal / "delivery.json").write_bytes(b"matching old controller evidence")
        flux = {"output_root": "/mnt/shared/flux-out/tl3", "staging_root": "/mnt/shared/tl3-inputs",
                "uri": "local:///fixture", "instance_generation": "fixture-generation",
                "runtime_bundles": ["/mnt/shared/flux-out/tl3/.runtime/" + "b" * 64 + "/manifest.json"]}
        text = ('execution_backend="flux"\nworktrees_root=' + json.dumps(str(self.root / "worktrees"))
                + '\n[store]\ndatabase="tl3"\nuser="tl3"\nport=5543\nhost='
                + json.dumps(str(self.root / "postgres/socket")) + '\n[workers]\nruns='
                + json.dumps(str(runs)) + '\n[flux]\n'
                + ''.join(key + '=' + deploy.toml_value(value) + '\n' for key, value in flux.items()))
        deploy.write_file(self.config / "tl3.toml", text.encode())
        plan = {"schema": "tl3.fresh-init.v1", "head": args.head,
                "config_sha256": hashlib.sha256(text.encode()).hexdigest(), "flux": flux,
                "archive": {"controller": str(journal.parent)}, "reset": ["controller/direct-merge"],
                "attempts": [], "tablespaces": {}}
        deploy.write_file(args.state_manifest, json.dumps(plan).encode())
        return args, plan, runs

    def test_actual_scope_uses_config_and_refuses_shared_parent_or_active_fx_erasure(self):
        args, plan, runs = self.scope_fixture()

        def transport(command, **kwargs):
            if list(map(str, command)) == ["git", "rev-parse", "HEAD"]:
                return args.head.encode()
            return b""

        with patch.object(deploy, "run", side_effect=transport), patch.object(deploy, "verify_runtime_inputs"):
            sources, reset, _ = deploy.fresh_scope(args)
            self.assertEqual(sources["controller"], runs / ".controller")
            self.assertEqual(reset, ["controller/direct-merge"])
            plan["reset"] = ["controller"]
            deploy.write_file(args.state_manifest, json.dumps(plan).encode())
            with self.assertRaisesRegex(RuntimeError, "exactly"):
                deploy.fresh_scope(args)
            plan["reset"] = ["controller/direct-merge"]
            deploy.write_file(args.state_manifest, json.dumps(plan).encode())
            active = runs / ".controller/flux-checks"
            active.mkdir(mode=0o700)
            (active / "bound.job").write_bytes(b"unresolved execution ownership")
            with self.assertRaisesRegex(RuntimeError, "cleanup"):
                deploy.fresh_scope(args)
            self.assertEqual((active / "bound.job").read_bytes(), b"unresolved execution ownership")

    def test_actual_scope_refuses_config_change_before_any_stop(self):
        args, _, _ = self.scope_fixture()
        (self.config / "tl3.toml").write_text('[store]\ndatabase="mlflow"\n')
        with self.assertRaisesRegex(RuntimeError, "bind exact"):
            deploy.fresh_scope(args)
        self.assertEqual(self.calls, [])

    def test_runtime_input_inventory_refuses_missing_tampered_or_unlisted_modules(self):
        bundle = self.home / "runtime"
        bundle.mkdir(mode=0o700)
        module = bundle / "runner.py"
        module.write_bytes(b"sealed runtime")
        module.chmod(0o400)
        (bundle / "manifest.json").write_text("fixture")
        manifest = {"schema": "tl3.runtime_bundle.v1", "files": [{"path": "runner.py", "mode": 0o400,
                    "bytes": 14, "sha256": hashlib.sha256(b"sealed runtime").hexdigest()}]}
        deploy.verify_runtime_files(bundle, manifest)
        module.unlink()
        with self.assertRaises(OSError):
            deploy.verify_runtime_files(bundle, manifest)
        module.write_bytes(b"tampered bytes")
        module.chmod(0o400)
        with self.assertRaisesRegex(RuntimeError, "mismatch"):
            deploy.verify_runtime_files(bundle, manifest)
        module.chmod(0o600)
        module.write_bytes(b"sealed runtime")
        module.chmod(0o400)
        (bundle / "unlisted.py").write_text("unexpected")
        with self.assertRaisesRegex(RuntimeError, "unlisted"):
            deploy.verify_runtime_files(bundle, manifest)


    def test_review_bad_backup_parent_refuses_before_service_stop(self):
        args, _, _ = self.scope_fixture()
        args.lane = "release"
        args.evidence = self.home
        args.backup.parent.mkdir(mode=0o775)
        args.backup.parent.chmod(0o775)
        with patch.object(deploy, "verify_runtime_inputs"):
            with self.assertRaises(RuntimeError):
                deploy.fresh_init(args)
        self.assertFalse(any(call[:3] == ["systemctl", "--user", "stop"] for call in self.calls))

    def test_review_source_symlink_refuses_before_service_stop(self):
        args, _, _ = self.scope_fixture()
        args.lane = "release"
        args.evidence = self.home
        (self.config / "unexpected-link").symlink_to(self.home)
        with patch.object(deploy, "verify_runtime_inputs"):
            with self.assertRaises(RuntimeError):
                deploy.fresh_init(args)
        self.assertFalse(any(call[:3] == ["systemctl", "--user", "stop"] for call in self.calls))

    def test_review_reset_does_not_rehash_unchanged_postgres_cluster(self):
        original = deploy.tree_manifest
        reads = []
        def inventory(path, *args, **kwargs):
            if path == self.cluster:
                reads.append(path)
            return original(path, *args, **kwargs)
        with patch.object(deploy, "tree_manifest", side_effect=inventory):
            deploy.cold_backup(self.sources, self.backup)
            deploy.reset_file_state(self.sources, self.backup, ("world", "journal"))
        self.assertLessEqual(len(reads), 1)

    def test_review_archived_context_cannot_authorize_drop_or_call_network_gate(self):
        deploy.cold_backup(self.sources, self.backup)
        for name in ("world", "journal"):
            for path in self.sources[name].iterdir():
                path.unlink()
        context = self.backup / "reset-context.json"
        fields = {"home": str(self.home), "head": "a" * 40, "lane": "release",
                  "evidence": str(self.home), "binaries": str(self.home / "binaries"),
                  "state_manifest": str(self.home / "state.json"), "backup": str(self.backup)}
        deploy.write_file(context, json.dumps(fields).encode())
        self.states["tl3-postgres.service"] = "active"
        with patch.object(deploy, "fresh_scope", return_value=(self.sources, (),
                                                               self.config / "tl3.toml")):
            with self.assertRaises(RuntimeError):
                deploy.authorize_fresh_database(context, self.config / "tl3.toml")
        self.assertFalse(any("qualify-fresh" in call for call in self.calls))


    def test_review_authorization_is_single_use_and_offline(self):
        deploy.cold_backup(self.sources, self.backup)
        context = self.backup / "reset-context.json"
        capability = b"e" * 64
        fields = {"home": str(self.home), "head": "a" * 40, "lane": "release",
                  "binaries": str(self.home / "binaries"), "state_manifest": str(self.home / "state.json"),
                  "backup": str(self.backup), "capability_sha256": hashlib.sha256(capability).hexdigest(),
                  "expires_monotonic": time.monotonic() + 500, "system_identifier": "12345"}
        deploy.write_file(context, json.dumps(fields).encode())
        self.states.update({unit: "inactive" for unit in
                            ("tl3d.service", "tl3-mlflow.service", "tl3-status.service", "tl3-console.service")})
        self.states["tl3-postgres.service"] = "active"
        with patch.object(deploy, "fresh_scope", return_value=(self.sources, (), self.config / "tl3.toml")):
            output = io.StringIO()
            with contextlib.redirect_stdout(output):
                deploy.authorize_fresh_database(context, self.config / "tl3.toml", capability)
            self.assertEqual(json.loads(output.getvalue()), {"system_identifier": "12345"})
            with self.assertRaisesRegex(RuntimeError, "consumed"):
                deploy.authorize_fresh_database(context, self.config / "tl3.toml", capability)
        self.assertFalse(any("qualify-fresh" in call for call in self.calls))
        self.assertNotIn(capability.decode(), context.read_text())

    def test_context_identity_mismatch_refuses_before_capability_consumption(self):
        deploy.cold_backup(self.sources, self.backup)
        context = self.backup / "reset-context.json"
        capability = b"e" * 64
        fields = {"home": str(self.home), "head": "a" * 40, "lane": "release",
                  "binaries": str(self.home / "selected"), "state_manifest": str(self.home / "scope.json"),
                  "backup": str(self.backup), "capability_sha256": hashlib.sha256(capability).hexdigest(),
                  "expires_monotonic": time.monotonic() + 500, "system_identifier": "99999"}
        deploy.write_file(context, json.dumps(fields).encode())
        self.states.update({unit: "inactive" for unit in deploy.CLIENT_UNITS})
        self.states["tl3-postgres.service"] = "active"
        with patch.object(deploy, "fresh_scope", return_value=(self.sources, (), self.config / "tl3.toml")):
            with self.assertRaisesRegex(RuntimeError, "context identity"):
                deploy.authorize_fresh_database(context, self.config / "tl3.toml", capability)
        self.assertFalse(context.with_suffix(".consumed").exists())

    def test_review_stability_detects_dependent_crash(self):
        self.states.update({unit: "active" for unit in
                            ("tl3d.service", "tl3-mlflow.service", "tl3-status.service", "tl3-console.service")})
        def crash(*unused):
            self.states["tl3-console.service"] = "failed"
        with patch.object(deploy.time, "sleep", side_effect=crash), \
             patch.object(deploy, "listener_ready", return_value=True):
            with self.assertRaisesRegex(RuntimeError, "console|stability"):
                deploy.verify_stability("127.0.0.1:8742")

    def test_review_binary_install_refuses_changed_source_bytes(self):
        args, _, _ = self.scope_fixture()
        (args.binaries / "tl3-deploy").write_bytes(b"changed after selection")
        with self.assertRaisesRegex(RuntimeError, "hash|binary|release"):
            deploy.install_selected_binaries(args.binaries, self.root)
        self.assertFalse((self.root / "bin/tl3-deploy").exists())


class DeployTests(unittest.TestCase):
    def setUp(self):
        self.scratch = tempfile.TemporaryDirectory(prefix="tl3-deploy-test-")
        self.addCleanup(self.scratch.cleanup)
        self.home = Path(self.scratch.name)
        self.helper = Path(os.environ["TL3_DEPLOY_HELPER"])

    def config(self):
        text = deploy.render_config(self.home, "192.168.1.107:8033", 987, 123,
                                    "sha256:" + "a" * 64, "gpt-test", "claude-test")
        config = self.home / "tl3.toml"
        config.write_text(text)
        return config, tomllib.loads(text)

    def test_generated_config_loads_and_status_needs_no_model(self):
        config, data = self.config()
        self.assertEqual(data["github"]["repos"], [{"owner": "RobTand", "name": "tl3-sandbox", "installation_id": 987}])
        self.assertEqual(data["operations"]["issue"], 123)
        self.assertEqual(data["store"]["host"], str(self.home / ".local/share/tl3/postgres/socket"))
        self.assertNotIn("default-model", config.read_text())
        for path in (data["github"]["app_credentials_file"], data["models"]["key_file"], data["store"]["password_file"]):
            deploy.write_file(Path(path), b"fixture only\n")
        result = subprocess.run([self.helper, "validate", config], capture_output=True)
        self.assertEqual(result.returncode, 0, result.stderr.decode())
        Path(data["models"]["key_file"]).unlink()
        result = subprocess.run([self.helper, "validate", config], capture_output=True)
        self.assertNotEqual(result.returncode, 0)

    def test_operations_rejects_zero(self):
        config, _ = self.config()
        config.write_text(config.read_text().replace("issue = 123", "issue = 0"))
        result = subprocess.run([self.helper, "parse", config], capture_output=True)
        self.assertNotEqual(result.returncode, 0)

    def test_lan_binding_rejects_wildcard_and_loopback(self):
        for address in ("0.0.0.0:8033", "127.0.0.1:8033", "192.168.1.107:0"):
            with self.assertRaises(RuntimeError):
                deploy.render_config(self.home, address)

    def test_password_rerun_never_reads_or_replaces_existing_secret(self):
        secret = self.home / "password"
        deploy.password(secret)
        info = secret.stat()
        self.assertEqual(stat.S_IMODE(info.st_mode), 0o600)
        with patch.object(Path, "read_text", side_effect=AssertionError("secret read")), \
             patch.object(Path, "read_bytes", side_effect=AssertionError("secret read")):
            deploy.password(secret)
        self.assertEqual(info.st_ino, secret.stat().st_ino)
        self.assertEqual(info.st_mtime_ns, secret.stat().st_mtime_ns)
        secret.chmod(0o644)
        with self.assertRaises(RuntimeError):
            deploy.password(secret)

    def test_download_verifies_hash_before_publication(self):
        destination = self.home / "cache/data"
        with patch.object(deploy.urllib.request, "urlopen", return_value=io.BytesIO(b"damaged")):
            with self.assertRaises(RuntimeError):
                deploy.checked_download("https://example.invalid", "0" * 64, destination)
        self.assertFalse(destination.exists())
        content = b"pinned fixture"
        with patch.object(deploy.urllib.request, "urlopen", return_value=io.BytesIO(content)):
            deploy.checked_download("https://example.invalid", hashlib.sha256(content).hexdigest(), destination)
        self.assertEqual(destination.read_bytes(), content)
        with patch.object(deploy.urllib.request, "urlopen", side_effect=AssertionError("unnecessary fetch")):
            deploy.checked_download("https://example.invalid", hashlib.sha256(content).hexdigest(), destination)

    def test_existing_data_never_gets_a_replacement_password(self):
        root, config, binaries = deploy.paths(self.home)
        (binaries / "bin").mkdir(parents=True)
        for name in deploy.PG_EXECUTABLES:
            deploy.write_file(deploy.pg_executable(binaries, name), b"#!/bin/sh\nexit 0\n", 0o700)
        (root / "postgres/data").mkdir(parents=True)
        with self.assertRaisesRegex(RuntimeError, "original password"):
            deploy.postgres(self.home)
        self.assertFalse((config / "secrets/postgres-password").exists())

    def test_service_uses_systemd_credentials_and_direct_python(self):
        config, data = self.config()
        deployed = self.home / ".config/tl3/tl3.toml"
        deploy.write_file(deployed, config.read_bytes())
        runtime = self.home / "runtime"
        credentials = self.home / "credentials"
        with patch.dict(os.environ, {"RUNTIME_DIRECTORY": str(runtime), "CREDENTIALS_DIRECTORY": str(credentials)}), \
             patch.object(os, "execv") as execute:
            deploy.service(self.home, "controller")
            self.assertEqual(os.environ["TIKTOKEN_CACHE_DIR"], data["workers"]["tiktoken_cache_dir"])
        runtime_config = tomllib.loads((runtime / "tl3.toml").read_text())
        self.assertEqual(runtime_config["hindsight"].get("api_key_file"), str(credentials / "hindsight-key"))
        self.assertIn("LoadCredential=hindsight-key:%h/.config/tl3/secrets/hindsight-key",
                      (deploy.HERE / "units/tl3d.service").read_text())
        self.assertEqual(runtime_config["models"]["key_file"], str(credentials / "model-key"))
        self.assertEqual(runtime_config["github"]["app_credentials_file"], str(credentials / "github-app.json"))
        self.assertEqual(execute.call_args.args[1][-2:], ["--config", str(runtime / "tl3.toml")])
        self.assertNotIn("uv", runtime_config["workers"]["python"])

    def test_unit_syntax_and_ordering(self):
        units = self.home / "units"
        units.mkdir()
        postgres_bin = deploy.paths(self.home)[2]
        for name in deploy.PG_EXECUTABLES:
            deploy.write_file(deploy.pg_executable(postgres_bin, name), b"#!/bin/sh\nexit 0\n", 0o700)
        for source in [*(deploy.HERE / "units").glob("*.service"), *(deploy.HERE / "units").glob("*.slice")]:
            lines = []
            for line in source.read_text().replace("%h", str(self.home)).splitlines():
                key, _, command = line.partition("=")
                if key in ("ExecStart", "ExecStartPre", "ExecStartPost", "ExecStop"):
                    executable = Path(command.split()[0])
                    if executable.is_relative_to(postgres_bin):
                        self.assertEqual(executable, deploy.pg_executable(postgres_bin, executable.name))
                    if not executable.is_relative_to(self.home):
                        # CI runners lack host binaries such as cloudflared, so every
                        # command runs a stub and verify checks syntax and ordering.
                        stub = self.home / "host" / executable.name
                        line = f"{key}={stub}{command[len(str(executable)):]}"
                        executable = stub
                    deploy.write_file(executable, b"#!/bin/sh\nexit 0\n", 0o700)
                lines.append(line)
            (units / source.name).write_text("\n".join(lines) + "\n")
        runtime = self.home / "runtime"
        runtime.mkdir(mode=0o700)
        environment = os.environ | {"XDG_RUNTIME_DIR": str(runtime)}
        result = subprocess.run(["systemd-analyze", "--user", "--man=no", "verify", *map(str, units.glob("*.service")),
                                 *map(str, units.glob("*.slice"))], capture_output=True, env=environment)
        self.assertEqual(result.returncode, 0, result.stderr.decode())
        controller = (units / "tl3d.service").read_text()
        self.assertIn("KillMode=control-group", controller)
        self.assertIn("Requires=tl3-postgres.service", controller)
        self.assertIn("After=tl3-postgres.service", controller.splitlines())
        self.assertIn(f"ConditionPathExists={self.home}/.config/tl3/production-ready", controller.splitlines())
        self.assertIn("LoadCredential=model-key:", controller)
        cloudflared = (units / "tl3-cloudflared.service").read_text()
        self.assertIn("--token-file %d/token", cloudflared)

    def test_private_credentials_require_nonempty_owner_only_regular_files(self):
        credential = self.home / "credential"
        with self.assertRaises(OSError):
            deploy.private_file(credential)
        deploy.write_file(credential, b"")
        with self.assertRaises(RuntimeError):
            deploy.private_file(credential)
        deploy.write_file(credential, b"fixture")
        deploy.private_file(credential)
        credential.chmod(0o644)
        with self.assertRaises(RuntimeError):
            deploy.private_file(credential)
        credential.chmod(0o600)
        link = self.home / "link"
        link.symlink_to(credential)
        with self.assertRaises(RuntimeError):
            deploy.private_file(link)
        with self.assertRaises(RuntimeError):
            deploy.private_file(self.home)

    def test_configure_says_which_flags_an_existing_config_ignores(self):
        config, _ = self.config()
        destination = self.home / ".config/tl3/tl3.toml"
        deploy.write_file(destination, config.read_bytes())
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            deploy.configure(self.home, "192.168.1.107:8033", {"installation_id": 987, "operations_issue": 123},
                             "sha256:" + "b" * 64, "replacement")
        self.assertIn("--image", output.getvalue())
        self.assertIn("--primary", output.getvalue())
        self.assertNotIn("--alternate", output.getvalue())
        self.assertEqual(destination.read_bytes(), config.read_bytes())

    def test_configure_explains_a_config_without_github_repos(self):
        destination = self.home / ".config/tl3/tl3.toml"
        deploy.write_file(destination, b"[store]\nhost = \"x\"\n")
        with self.assertRaisesRegex(SystemExit, r"\[\[github\.repos\]\]"):
            deploy.configure(self.home, "192.168.1.107:8033", {"installation_id": 987})

    def test_model_defaults_are_explicit_and_preserve_endpoint_and_key_path(self):
        config, data = self.config()
        destination = self.home / ".config/tl3/tl3.toml"
        deploy.write_file(destination, config.read_bytes())
        deploy.model_defaults(self.home)
        actual = tomllib.loads(destination.read_text())["models"]
        self.assertEqual(actual["base_url"], data["models"]["base_url"])
        self.assertEqual(actual["key_file"], data["models"]["key_file"])
        defaults = tomllib.loads((deploy.HERE / "models.toml").read_text())["models"]
        for role, ladder in defaults.items():
            self.assertEqual(actual[role], ladder)
        # #416: attempt 2 escalates to Opus 5.5 at high effort.
        self.assertEqual(actual["maker"][1]["name"], "claude-opus-5-5")
        self.assertEqual(actual["maker"][1]["effort"], "high")
        self.assertEqual(actual["maker"][1]["budget_tokens"], 128000)

    def binary_bundle(self, directory):
        directory.mkdir(parents=True, exist_ok=True)
        sums = []
        for name in ("tl3", "tl3d", "tl3-deploy", "tl3-worker-supervisor"):
            content = f"release fixture for {name}\n".encode()
            deploy.write_file(directory / name, content, 0o700)
            sums.append(f"{hashlib.sha256(content).hexdigest()}  {name}\n")
        deploy.write_file(directory / "SHA256SUMS", "".join(sums).encode())

    def checked_bundle(self, damage=None):
        root, config, _ = deploy.paths(self.home)
        self.binary_bundle(root / "bin")
        if damage:
            damage(root / "bin")
        source, _ = self.config()
        deploy.write_file(config / "tl3.toml", source.read_bytes())
        for name in ("model-key", "hindsight-key"):
            deploy.write_file(config / "secrets" / name, b"fixture")
        deploy.write_file(config / "production-ready", b"")
        with patch.object(deploy, "mlflow_health", return_value=True), \
             patch.object(deploy, "launcher_works", return_value=(True, "fixture")), \
             patch.object(subprocess, "run", return_value=subprocess.CompletedProcess([], 0)), \
             patch.object(deploy.http.client, "HTTPConnection") as http, \
             contextlib.redirect_stdout(io.StringIO()) as output:
            http.return_value.getresponse.side_effect = [
                unittest.mock.Mock(status=200),
                unittest.mock.Mock(status=405),
            ]
            result = deploy.check(self.home)
        return result, output.getvalue()

    def test_check_accepts_complete_binary_bundle(self):
        result, output = self.checked_bundle()
        self.assertEqual(result, 0, output)

    def test_check_rejects_truncated_binary(self):
        result, output = self.checked_bundle(lambda directory: (directory / "tl3d").write_bytes(b""))
        self.assertEqual(result, 1, output)
        self.assertIn("MISSING bin/tl3d", output)

    def test_check_rejects_missing_binary(self):
        result, output = self.checked_bundle(lambda directory: (directory / "tl3-deploy").unlink())
        self.assertEqual(result, 1, output)
        self.assertIn("MISSING bin/tl3-deploy", output)

    def test_check_rejects_stale_binary(self):
        result, output = self.checked_bundle(
            lambda directory: (directory / "tl3-worker-supervisor").write_bytes(b"older release"))
        self.assertEqual(result, 1, output)
        self.assertIn("MISSING bin/tl3-worker-supervisor", output)

    def test_check_ignores_binaries_owned_by_other_deployers(self):
        # tl3-console is installed into bin/ by RobTand/tl3-console's own deploy and is not
        # in TL3's release SHA256SUMS; TL3's check must neither verify nor flag it (#784 regression).
        for name in ("tl3-console", "old-worker"):
            result, output = self.checked_bundle(
                lambda directory, name=name: deploy.write_file(directory / name, b"other deployer", 0o700))
            self.assertEqual(result, 0, output)
            self.assertNotIn(f"bin/{name}", output)

    def test_check_requires_every_release_binary_in_the_manifest(self):
        def drop(directory):
            sums = (directory / "SHA256SUMS").read_text().splitlines(keepends=True)
            (directory / "SHA256SUMS").write_text("".join(line for line in sums if not line.rstrip().endswith(" tl3d")))
        result, output = self.checked_bundle(drop)
        self.assertGreaterEqual(result, 1, output)
        self.assertIn("MISSING bin/tl3d", output)

    def test_failed_binary_skips_only_checks_that_invoke_it(self):
        result, output = self.checked_bundle(lambda directory: (directory / "tl3").write_bytes(b"stale"))
        self.assertIn("MISSING bin/tl3:", output)
        self.assertIn("MISSING tl3 status: binary verification failed", output)
        self.assertNotIn("Postgres reachable and migrated: binary verification failed", output)
        self.assertNotIn("Config::load including secret metadata: binary verification failed", output)
        self.assertEqual(result, 1, output)
        self.assertIn("Deployment check: 2 missing checks", output)

    def test_install_preserves_release_binary_manifest(self):
        binaries = self.home / "release"
        self.binary_bundle(binaries)
        args = deploy.argparse.Namespace(home=self.home, binaries=binaries,
                                        status_listen="192.168.1.107:8033", image=None,
                                        primary=None, alternate=None, console_listen=None,
                                        managed_tunnel=False)
        with patch.object(deploy, "postgres"), patch.object(deploy, "checked_download"), \
             patch.object(deploy, "install_release", return_value=False), \
             patch.object(deploy, "configure"), patch.object(deploy, "grant_console_reader"), \
             patch.object(deploy, "collect_releases"), patch.object(deploy, "run"), \
             contextlib.redirect_stdout(io.StringIO()):
            deploy.install(args)
        installed = deploy.paths(self.home)[0] / "bin/SHA256SUMS"
        self.assertTrue(installed.is_file(), "install must retain the release SHA256SUMS")
        self.assertEqual(installed.read_bytes(), (binaries / "SHA256SUMS").read_bytes())

    def test_check_reports_both_bad_credentials_as_missing(self):
        config, data = self.config()
        deploy.write_file(deploy.paths(self.home)[0] / "deploy/managed-tunnel", b"managed\n")
        destination = self.home / ".config/tl3/tl3.toml"
        deploy.write_file(destination, config.read_bytes())
        key = destination.parent / "secrets/model-key"
        token = destination.parent / "secrets/cloudflared-token"
        # tl3d's unit loads hindsight-key unconditionally, so a missing file stops the service.
        hindsight = destination.parent / "secrets/hindsight-key"
        for content, mode in ((b"", 0o600), (b"fixture", 0o644), (b"fixture", 0o600)):
            for path in (key, token, hindsight):
                deploy.write_file(path, content, mode)
            with patch.object(subprocess, "run", return_value=subprocess.CompletedProcess([], 0)), \
                 patch.object(deploy.http.client, "HTTPConnection"), \
                 contextlib.redirect_stdout(io.StringIO()) as output:
                deploy.check(self.home)
            prefix = "OK" if content and mode == 0o600 else "MISSING"
            self.assertIn(f"{prefix} model credential\n", output.getvalue())
            self.assertIn(f"{prefix} Cloudflare token\n", output.getvalue())
            self.assertIn(f"{prefix} Hindsight credential\n", output.getvalue())

    def test_external_tunnel_needs_no_token_or_tl3_connector(self):
        config, _ = self.config()
        destination = self.home / ".config/tl3/tl3.toml"
        deploy.write_file(destination, config.read_bytes())
        deploy.write_file(destination.parent / "secrets/model-key", b"fixture")
        deploy.write_file(destination.parent / "production-ready", b"")
        calls = []

        def command(argv, **kwargs):
            calls.append(list(map(str, argv)))
            return subprocess.CompletedProcess(argv, 0)

        with patch.object(subprocess, "run", side_effect=command), \
             patch.object(deploy.http.client, "HTTPConnection") as http, \
             contextlib.redirect_stdout(io.StringIO()) as output:
            http.return_value.getresponse.return_value.status = 200
            deploy.check(self.home)
        self.assertNotIn("Cloudflare token", output.getvalue())
        self.assertFalse(any("tl3-cloudflared" in call for call in calls))
        self.assertIn("external webhook route", output.getvalue())

    def test_tl3_leaves_the_console_to_its_own_repository(self):
        # PRD #327: RobTand/tl3-console builds, installs and runs tl3-console.service.
        # TL3 keeps the board projection and the console's read-only login.
        self.assertFalse((deploy.HERE / "units/tl3-console.service").exists())
        self.assertNotIn("tl3-console", (deploy.HERE / "build-release.sh").read_text())
        self.assertFalse(hasattr(deploy, "build_console_assets"))
        config, _ = self.config()
        deploy.write_file(self.home / ".config/tl3/tl3.toml", config.read_bytes())
        runtime, credentials = self.home / "runtime", self.home / "credentials"
        with patch.dict(os.environ, {"RUNTIME_DIRECTORY": str(runtime), "CREDENTIALS_DIRECTORY": str(credentials)}), \
             patch.object(os, "execv") as execv, self.assertRaises(RuntimeError):
            deploy.service(self.home, "console")
        execv.assert_not_called()
        calls, probes = [], []

        def command(argv, **kwargs):
            calls.append(list(map(str, argv)))
            return subprocess.CompletedProcess(argv, 0)

        def connection(host, port, **kwargs):
            probes.append(f"{host}:{port}")
            return unittest.mock.MagicMock()

        with patch.object(subprocess, "run", side_effect=command), \
             patch.object(deploy.http.client, "HTTPConnection", side_effect=connection), \
             contextlib.redirect_stdout(io.StringIO()) as output:
            deploy.check(self.home)
        self.assertFalse(any("tl3-console" in part for call in calls for part in call))
        self.assertNotIn("console", output.getvalue())
        self.assertNotIn("192.168.1.107:8034", probes)

    def test_controller_stops_its_whole_process_group_and_preserves_gate(self):
        unit = (deploy.HERE / "units/tl3d.service").read_text().splitlines()
        self.assertIn("KillMode=control-group", unit)
        self.assertIn("ConditionPathExists=%h/.config/tl3/production-ready", unit)

    def test_generated_config_serves_the_console_on_the_lan(self):
        _, data = self.config()
        self.assertEqual(data["console"]["listen"], "192.168.1.107:8034")
        self.assertEqual(data["console"]["assets"], str(self.home / ".local/share/tl3/console"))
        with self.assertRaises(RuntimeError):
            deploy.render_config(self.home, "192.168.1.107:8033", console_listen="127.0.0.1:8034")

    def test_fresh_install_uses_the_console_listen_flag(self):
        destination = self.home / ".config/tl3/tl3.toml"
        deploy.configure(self.home, "192.168.1.107:8033", {"installation_id": 987},
                         console_listen="192.168.1.107:9100")
        self.assertEqual(tomllib.loads(destination.read_text())["console"]["listen"], "192.168.1.107:9100")

    def test_console_connects_as_a_read_only_login(self):
        _, data = self.config()
        config = self.home / ".config/tl3"
        self.assertEqual(data["console"]["user"], "tl3_console")
        self.assertEqual(data["console"]["password_file"], str(config / "secrets/console-postgres-password"))
        destination = config / "tl3.toml"
        text = self.config()[0].read_text()
        deploy.write_file(destination, text.encode())
        deploy.configure(self.home, "192.168.1.107:8033", {})
        self.assertEqual(destination.read_text(), text)
        # A missing console credential must never stop Postgres, which tl3d and status require.
        self.assertNotIn("console-postgres-password", (deploy.HERE / "units/tl3-postgres.service").read_text())

    def test_postgres_setup_creates_the_console_credential_once(self):
        root, config, binaries = deploy.paths(self.home)
        (binaries / "bin").mkdir(parents=True)
        for name in deploy.PG_EXECUTABLES:
            deploy.write_file(deploy.pg_executable(binaries, name), b"#!/bin/sh\nexit 0\n", 0o700)
        with contextlib.redirect_stdout(io.StringIO()):
            deploy.postgres(self.home)
        secret = config / "secrets/console-postgres-password"
        deploy.private_file(secret)
        first = secret.read_bytes()
        self.assertNotEqual(first, (config / "secrets/postgres-password").read_bytes())
        with contextlib.redirect_stdout(io.StringIO()):
            deploy.postgres(self.home)
        self.assertEqual(secret.read_bytes(), first)

    def test_database_reads_the_console_credential_in_place(self):
        config, _ = self.config()
        destination = self.home / ".config/tl3/tl3.toml"
        deploy.write_file(destination, config.read_bytes())
        runtime = self.home / "runtime"
        credentials = self.home / "credentials"
        secret = self.home / ".config/tl3/secrets/console-postgres-password"
        with patch.dict(os.environ, {"RUNTIME_DIRECTORY": str(runtime), "CREDENTIALS_DIRECTORY": str(credentials)}), \
             patch.object(os, "execv"):
            deploy.service(self.home, "database")
        written = tomllib.loads((runtime / "tl3.toml").read_text())
        # The database service reads the console secret in place; the grant is best effort there.
        self.assertEqual(written["console"]["password_file"], str(secret))

    def test_install_grants_the_console_login_when_postgres_runs(self):
        root, config, _ = deploy.paths(self.home)
        for active in (True, False):
            calls = []

            def command(argv, **kwargs):
                calls.append([str(part) for part in argv])
                return subprocess.CompletedProcess(argv, 0 if active else 3)

            with self.subTest(active=active), patch.object(subprocess, "run", side_effect=command), \
                 patch.object(deploy, "run", side_effect=lambda argv, **kwargs: calls.append([str(p) for p in argv])), \
                 contextlib.redirect_stdout(io.StringIO()) as output:
                deploy.grant_console_reader(self.home)
            grant = [str(root / "bin/tl3-deploy"), "console-role", str(config / "tl3.toml")]
            self.assertEqual(grant in calls, active)
            if not active:
                self.assertIn("tl3-postgres", output.getvalue())

    def release_source(self, version):
        source = self.home / f"source-{version}"
        (source / "workers/src/tl3_workers").mkdir(parents=True, exist_ok=True)
        (source / "harness").mkdir(exist_ok=True)
        (source / "workers/pyproject.toml").write_text("[project]\nname = 'tl3-workers'\n")
        (source / "workers/src/tl3_workers/__init__.py").write_text(f"VERSION = {version}\n")
        (source / "harness/runner.py").write_text("# harness\n")
        # Ignored build products never change a release's identity.
        (source / "workers/.venv").mkdir(exist_ok=True)
        (source / "workers/.venv/marker").write_text(str(version))
        return source

    def install_release(self, source):
        synced = []

        def fake_sync(argv, **kwargs):
            project = Path(argv[argv.index("--project") + 1])
            (project / ".venv/bin").mkdir(parents=True)
            (project / ".venv/bin/python").symlink_to("/usr/bin/python3")
            synced.append(project)

        with patch.object(deploy, "run", side_effect=fake_sync), contextlib.redirect_stdout(io.StringIO()):
            introduced = deploy.install_release(self.home, source)
        return introduced, synced

    def collect(self, introduced=False, runtime=None):
        environment = {"XDG_RUNTIME_DIR": str(runtime or self.home / "no-runtime")}
        with patch.dict(os.environ, environment), contextlib.redirect_stdout(io.StringIO()) as output:
            deploy.collect_releases(self.home)
        return output.getvalue()

    def live_attempt(self, python, *, launch=True):
        runs = deploy.paths(self.home)[0] / "worker-runs"
        attempt = runs / f"run{len(list(runs.glob('*')))}" / "attempt-00000000"
        attempt.mkdir(parents=True)
        stat = Path("/proc/self/stat").read_text().rsplit(")", 1)[1].split()
        (attempt / "claim.json").write_text(json.dumps({
            "pid": os.getpid(), "start": int(stat[19]),
            "machine": Path("/etc/machine-id").read_text().strip(),
            "boot": Path("/proc/sys/kernel/random/boot_id").read_text().strip()}))
        if launch:
            (attempt / "launch.json").write_text(json.dumps({"python": str(python)}))
        return attempt

    def test_running_worker_keeps_its_release_across_deploys(self):
        root = deploy.paths(self.home)[0]
        root.mkdir(parents=True)
        self.assertTrue(self.install_release(self.release_source(1))[0], "the first install introduces releases")
        first = (root / "current").resolve()
        self.live_attempt(first / "workers/.venv/bin/python")
        self.assertFalse(self.install_release(self.release_source(2))[0])
        second = (root / "current").resolve()
        self.assertFalse((root / "previous").exists())
        # The running worker's release is untouched; only `current` moved.
        self.assertEqual((first / "workers/src/tl3_workers/__init__.py").read_text(), "VERSION = 1\n")
        self.assertEqual((second / "workers/src/tl3_workers/__init__.py").read_text(), "VERSION = 2\n")
        self.assertFalse((second / "workers/.venv/marker").exists())
        self.install_release(self.release_source(3))
        self.collect()
        self.assertEqual({path.resolve() for path in (root / "releases").iterdir()},
                         {first, (root / "current").resolve()},
                         "keep current and the live run's release")
        for record in (root / "worker-runs").glob("*/attempt-*/claim.json"):
            record.unlink()
        self.install_release(self.release_source(4))
        self.collect()
        self.assertEqual({path.resolve() for path in (root / "releases").iterdir()},
                         {(root / "current").resolve()})

    def test_redeploying_the_same_code_reuses_its_release(self):
        root = deploy.paths(self.home)[0]
        root.mkdir(parents=True)
        self.install_release(self.release_source(1))
        first = (root / "current").resolve()
        source = self.release_source(2)
        self.install_release(source)
        release = (root / "current").resolve()
        before = (release / ".installed").stat().st_mtime_ns
        introduced, synced = self.install_release(source)
        self.assertFalse(introduced)
        self.assertEqual(synced, [], "no copy or sync over a running release")
        self.assertEqual((root / "current").resolve(), release)
        self.assertEqual((release / ".installed").stat().st_mtime_ns, before)
        self.assertFalse((root / "previous").exists())
        self.collect()
        self.assertFalse(first.exists())

    def test_unfinished_release_is_rebuilt_not_used(self):
        root = deploy.paths(self.home)[0]
        root.mkdir(parents=True)
        source = self.release_source(1)
        partial = root / "releases" / deploy.release_id(source)
        (partial / "workers").mkdir(parents=True)
        (partial / "workers/stale.py").write_text("partial copy\n")
        _, synced = self.install_release(source)
        self.assertEqual(synced, [partial / "workers"])
        self.assertFalse((partial / "workers/stale.py").exists())
        self.assertTrue((partial / ".installed").exists())

    def test_supervisors_launch_outside_tl3d_from_the_current_release(self):
        config, _ = self.config()
        workers = tomllib.loads(config.read_text())["workers"]
        self.assertEqual(workers["launcher"][:3], ["/usr/bin/systemd-run", "--user", "--scope"])
        self.assertIn("--slice=tl3-workers.slice", workers["launcher"])
        self.assertEqual(workers["python"], str(deploy.paths(self.home)[0] / "current/workers/.venv/bin/python"))
        self.assertTrue((deploy.HERE / "units" / deploy.WORKER_SLICE).is_file())
        unit = (deploy.HERE / "units/tl3d.service").read_text().splitlines()
        self.assertIn("KillMode=control-group", unit, "tl3d's own cgroup still stops on its own")

    def test_stop_runs_refuses_unless_tl3d_is_inactive_or_failed(self):
        for state, allowed in (("active", False), ("activating", False), ("reloading", False),
                               ("deactivating", False), ("inactive", True), ("failed", True)):
            calls = []
            with self.subTest(state=state), patch.object(deploy, "_tl3d_state", return_value=state), \
                 patch.object(deploy, "run", side_effect=lambda argv, **kwargs: calls.append([str(p) for p in argv])), \
                 contextlib.redirect_stdout(io.StringIO()):
                if allowed:
                    deploy.stop_runs(self.home)
                else:
                    with self.assertRaisesRegex(RuntimeError, "Stop tl3d first"):
                        deploy.stop_runs(self.home)
            self.assertEqual(calls, [["systemctl", "--user", "stop", "tl3-workers.slice"]] if allowed else [])

    def probe_config(self, launcher):
        _, config_dir, _ = deploy.paths(self.home)
        config, _ = self.config()
        text = config.read_text().replace("launcher = " + deploy.toml_value(deploy.LAUNCHER),
                                          "launcher = " + deploy.toml_value(launcher))
        deploy.write_file(config_dir / "tl3.toml", text.encode())

    def probe(self, *, timeout=20):
        runtime = self.home / "runtime"
        runtime.mkdir(exist_ok=True)
        bus = runtime / "bus"
        if not bus.exists():
            server = socket.socket(socket.AF_UNIX)
            self.addCleanup(server.close)
            server.bind(str(bus))
        with patch.dict(os.environ, {"XDG_RUNTIME_DIR": str(runtime)}), \
             patch.object(deploy, "PROBE_TIMEOUT", timeout, create=True):
            return deploy.launcher_works(self.home)

    def fake_launcher(self, body):
        script = self.home / "fake-launcher"
        script.write_text("#!/bin/sh\n" + body + "\n")
        script.chmod(0o700)
        return [str(script)]

    def test_launcher_probe_runs_the_configured_launcher_and_reports_why_it_failed(self):
        self.probe_config(self.fake_launcher('echo "Failed to connect to user scope bus" >&2; exit 1'))
        ok, detail = self.probe()
        self.assertFalse(ok)
        self.assertIn("Failed to connect to user scope bus", detail, "the configured launcher ran, not the constant")

    def test_launcher_probe_is_bounded(self):
        self.probe_config(self.fake_launcher("exec sleep 30"))
        started = time.monotonic()
        ok, detail = self.probe(timeout=1)
        self.assertFalse(ok)
        self.assertIn("timed out", detail)
        self.assertLess(time.monotonic() - started, 15)

    def test_launcher_probe_needs_a_scope_of_its_own_and_a_working_cgroup_kill(self):
        # A launcher that runs the command in place leaves it in the caller's cgroup.
        self.probe_config(["/usr/bin/env"])
        ok, detail = self.probe()
        self.assertFalse(ok)
        self.assertIn("scope of its own", detail)
        pid_file = self.home / "probe.pid"
        self.probe_config(self.fake_launcher(
            f'echo $$ > {pid_file}; echo "0::/user.slice/user@1000.service/tl3.slice/tl3-workers.slice/run-probe.scope"; '
            "exec sleep 30"))
        killed = []

        def cgroup_kill(scope):
            killed.append(scope)
            os.kill(int(pid_file.read_text()), 9)

        with patch.object(deploy, "_kill_cgroup", side_effect=cgroup_kill):
            ok, detail = self.probe()
        self.assertTrue(ok, detail)
        self.assertEqual(killed, [Path("/sys/fs/cgroup/user.slice/user@1000.service/tl3.slice/tl3-workers.slice/run-probe.scope")])
        with patch.object(deploy, "_kill_cgroup", side_effect=lambda scope: None):
            ok, detail = self.probe(timeout=1)
        self.assertFalse(ok)
        self.assertIn("cgroup.kill", detail)

    def test_launcher_probe_needs_the_user_bus_and_a_configured_launcher(self):
        with patch.dict(os.environ, {"XDG_RUNTIME_DIR": str(self.home / "nothing")}):
            ok, detail = deploy.launcher_works(self.home)
        self.assertFalse(ok)
        self.assertIn("user bus", detail)
        self.probe_config([])
        ok, detail = self.probe()
        self.assertFalse(ok)
        self.assertIn("workers.launcher", detail)

    def test_command_failure_never_prints_child_output(self):
        with contextlib.redirect_stdout(io.StringIO()) as output:
            with self.assertRaisesRegex(RuntimeError, "output suppressed"):
                deploy.run(["sh", "-c", "echo secret-output; exit 7"])
        self.assertEqual(output.getvalue(), "")

    def test_command_timeout_bounds_the_owned_child_process_group(self):
        with self.assertRaisesRegex(RuntimeError, "timed out"):
            deploy.run(["sh", "-c", "sleep 10 & wait"], timeout=0.05)


class ExternalCallerTests(unittest.TestCase):
    """Real Git fixtures; fake qualification proves control flow, not eligibility."""

    def setUp(self):
        self.scratch = tempfile.TemporaryDirectory(prefix="tl3-caller-")
        self.addCleanup(self.scratch.cleanup)
        self.base = Path(self.scratch.name)
        self.source = self.base / "unrelated-checkout"
        self.source.mkdir()
        self.env = {k: v for k, v in os.environ.items() if not k.startswith("GIT_")}
        self.env.update(GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL="/dev/null",
                        GIT_TERMINAL_PROMPT="0")
        self.git("init", "-q")
        self.git("commit", "-q", "--allow-empty", "-m", "unrelated checkout without deploy")
        self.unrelated = self.git("rev-parse", "HEAD").strip().decode()
        directory = self.source / "deploy/dl380g10"
        directory.mkdir(parents=True)
        (directory / "deploy-main.sh").write_bytes(Path(__file__).with_name("deploy-main.sh").read_bytes())
        # A harmless qualification command records exact cwd/head/tree then
        # refuses. Real deploy.py's authoritative gate is exercised separately.
        (directory / "deploy.py").write_text(
            "import json,os,subprocess,sys\nfrom pathlib import Path\n"
            "Path(os.environ['CALLER_RECORD']).write_text(json.dumps({"
            "'cwd':os.getcwd(),'head':subprocess.check_output(['git','rev-parse','HEAD']).decode().strip(),"
            "'tree':subprocess.check_output(['git','rev-parse','HEAD^{tree}']).decode().strip(),"
            "'argv':sys.argv[1:]}))\nraise SystemExit(37)\n")
        self.git("add", ".")
        self.git("commit", "-q", "-m", "exact release")
        self.head = self.git("rev-parse", "HEAD").strip().decode()
        self.tree = self.git("rev-parse", "HEAD^{tree}").strip().decode()
        self.git("checkout", "-q", "--detach", self.unrelated)
        (self.source / "unrelated-dirt").write_text("must never be read or cleaned")
        self.root = self.base / "owned-sources"
        self.record = self.base / "record.json"
        self.env.update(CALLER_RECORD=str(self.record), TL3_FRESH_STATE_MANIFEST="unused",
                        TL3_COLD_BACKUP="unused", TL3_DEPLOY_DRY_RUN="1")
        self.caller = self.base / "external.sh"
        text = Path(__file__).with_name("external-deploy-main.sh").read_text()
        text = text.replace("https://github.com/RobTand/trustladderiii.git", str(self.source))
        text = text.replace("/home/rob/.local/share/tl3-deploy-sources", str(self.root))
        if os.environ.get("TL3_CALLER_RED") == "1":
            # Committed minimal model of the installed cd22d3b8 caller. The
            # historical production checkout is never read or executed.
            text = '#!/bin/bash\nset -euo pipefail\nrepo=' + str(self.source) + '\nexec bash "$repo/deploy/dl380g10/deploy-main.sh" "$@"\n'
        self.caller.write_text(text)

    def git(self, *args):
        return subprocess.check_output(["git", "-c", "user.name=Fixture", "-c",
                                       "user.email=fixture@example.invalid", "-c",
                                       "commit.gpgsign=false", *args], cwd=self.source,
                                      env=self.env, stderr=subprocess.PIPE, timeout=15)

    def call(self, *args, overrides=None):
        return subprocess.run(["bash", str(self.caller), *args], env=self.env | (overrides or {}),
                              capture_output=True, timeout=90)

    def assert_refused(self, result):
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(self.record.exists(), result.stderr.decode())

    def test_exact_release_independent_of_absent_unrelated_checkout_target(self):
        self.assertFalse((self.source / "deploy/dl380g10/deploy-main.sh").exists())
        result = self.call(self.head, overrides={"GIT_DIR": "/absent", "GIT_WORK_TREE": "/absent",
                                               "GIT_CONFIG_COUNT": "1", "GIT_CONFIG_KEY_0": "core.bare",
                                               "GIT_CONFIG_VALUE_0": "true"})
        self.assertEqual(result.returncode, 37, result.stderr.decode())
        record = json.loads(self.record.read_text())
        self.assertEqual(record, {"head": self.head, "tree": self.tree,
                                "cwd": str(self.root / self.head),
                                "argv": ["qualify-fresh", "--head", self.head, "--lane", "release"]})
        self.assertEqual(self.git("rev-parse", "HEAD").strip().decode(), self.unrelated)
        self.assertTrue((self.source / "unrelated-dirt").exists())
        self.record.unlink()
        self.assertEqual(self.call(self.head).returncode, 37)  # safe clean reuse

    def test_missing_short_branch_and_extra_inputs_refuse_before_fetch(self):
        for args in [(), ("main",), (self.head[:12],), (self.head, "extra")]:
            with self.subTest(args=args):
                self.assert_refused(self.call(*args))
                self.assertFalse(self.root.exists())

    def test_missing_source_object_refuses_without_publication(self):
        self.assert_refused(self.call("f" * 40))
        self.assertFalse((self.root / ("f" * 40)).exists())

    def test_tracked_staged_untracked_and_ignored_dirt_refuse(self):
        self.assertEqual(self.call(self.head).returncode, 37)
        self.record.unlink()
        tree = self.root / self.head
        for kind in ("tracked", "staged", "untracked", "ignored"):
            with self.subTest(kind=kind):
                path = tree / ("deploy/dl380g10/deploy.py" if kind in ("tracked", "staged") else "dirt")
                original = path.read_bytes() if path.exists() else None
                path.write_text("dirty fixture")
                if kind == "staged":
                    subprocess.run(["git", "add", "."], cwd=tree, env=self.env, check=True, timeout=15)
                if kind == "ignored":
                    (tree / ".git/info/exclude").write_text("dirt\n")
                self.assert_refused(self.call(self.head))
                self.assertEqual(path.read_text(), "dirty fixture")
                # Fixture-only restoration, never production cleanup.
                if original is None:
                    path.unlink()
                else:
                    path.write_bytes(original)
                subprocess.run(["git", "reset", "-q", "HEAD"], cwd=tree, env=self.env, check=True, timeout=15)

    def test_mismatched_existing_tree_and_symlink_refuse(self):
        self.assertEqual(self.call(self.head).returncode, 37)
        self.record.unlink()
        tree = self.root / self.head
        subprocess.run(["git", "checkout", "-q", "--detach", self.unrelated], cwd=tree,
                       env=self.env, check=True, timeout=15)
        self.assert_refused(self.call(self.head))
        moved = self.root / "moved"
        tree.rename(moved)
        tree.symlink_to(moved, target_is_directory=True)
        self.assert_refused(self.call(self.head))

    def test_canonical_direct_mismatch_and_dirty_refuse_before_gate(self):
        self.assertEqual(self.call(self.head).returncode, 37)
        self.record.unlink()
        tree = self.root / self.head
        caller = tree / "deploy/dl380g10/deploy-main.sh"
        result = subprocess.run(["bash", str(caller), self.unrelated], env=self.env,
                                capture_output=True, timeout=30)
        self.assert_refused(result)
        (tree / "dirt").write_text("dirty")
        result = subprocess.run(["bash", str(caller), self.head], env=self.env,
                                capture_output=True, timeout=30)
        self.assert_refused(result)

    def test_shallow_and_wrong_upstream_existing_clones_refuse(self):
        self.assertEqual(self.call(self.head).returncode, 37)
        self.record.unlink()
        tree = self.root / self.head
        (tree / ".git/shallow").write_text(self.head + "\n")
        self.assert_refused(self.call(self.head))
        (tree / ".git/shallow").unlink()
        subprocess.run(["git", "remote", "set-url", "origin", str(self.base / "absent")],
                       cwd=tree, env=self.env, check=True, timeout=15)
        self.assert_refused(self.call(self.head))

    def test_unavailable_upstream_refuses_without_published_tree(self):
        text = self.caller.read_text().replace(str(self.source), str(self.base / "absent"))
        self.caller.write_text(text)
        self.assert_refused(self.call(self.head))
        self.assertFalse((self.root / self.head).exists())


if __name__ == "__main__":
    unittest.main()
