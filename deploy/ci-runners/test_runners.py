"""Runner command boundaries. Execute through PrismaBuild."""

import configparser
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parent
IMAGE = "sha256:" + "a" * 64
JIT = "synthetic-one-job-config"


class RunnerTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.home = Path(self.temp.name)
        self.bin = self.home / "bin"
        self.bin.mkdir()
        self.calls = self.home / "calls.jsonl"
        self.env = {**os.environ, "HOME": str(self.home),
                    "PATH": f"{self.bin}:{os.environ['PATH']}",
                    "CALLS": str(self.calls), "CI_RUNNER_IMAGE": IMAGE}
        self.env.pop("CI_RUNNER_CPUSET", None)
        for tool in ("gh", "podman", "systemctl", "systemd-notify", "uname", "id"):
            path = self.bin / tool
            path.write_text("#!/usr/bin/env python3\n" + '''import json, os, sys
from pathlib import Path
name = Path(sys.argv[0]).name
args = sys.argv[1:]
with open(os.environ["CALLS"], "a") as file:
    file.write(json.dumps([name, *args]) + "\\n")
if name == "gh":
    print("synthetic-one-job-config")
elif name == "podman":
    if args[0] == "info": print("true")
    elif args[:2] == ["image", "inspect"]:
        key = "NEW_IMAGE" if (Path(os.environ["HOME"]) / "built").exists() else "OLD_IMAGE"
        print(os.environ.get(key, "sha256:" + "a" * 64))
    elif args[0] == "build": (Path(os.environ["HOME"]) / "built").touch()
    elif args[:2] == ["image", "rm"]:
        for config in (Path(os.environ["HOME"]) / ".config/tl3-ci-runners").glob("*.env"):
            assert "CI_RUNNER_IMAGE=" + args[2] not in config.read_text().splitlines(), config
    elif args[0] == "run":
        assert sys.stdin.read() == "synthetic-one-job-config\\n"
elif name == "uname": print("x86_64")
elif name == "id": print("1000")
''')
            path.chmod(0o700)

    def run_script(self, script, *args):
        return subprocess.run(["bash", str(ROOT / script), *args],
                              env=self.env, text=True, capture_output=True)

    def commands(self):
        return [json.loads(line) for line in self.calls.read_text().splitlines()]

    def assert_slot(self, profile, repository, label, prefix):
        result = self.run_script("runner-slot.sh", "1", profile)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotIn(JIT, result.stdout + result.stderr)
        commands = self.commands()
        registration = next(c for c in commands if c[0] == "gh")
        self.assertIn(f"repos/RobTand/{repository}/actions/runners/generate-jitconfig", registration)
        self.assertIn(f"labels[]={label}", registration)
        container = next(c for c in commands if c[:2] == ["podman", "run"])
        self.assertEqual(container[container.index("--name") + 1], f"{prefix}-1")
        for option in ("--cpus=4", "--memory=6g", "--memory-swap=6g", "--sdnotify=ignore",
                       "--cap-drop=all", "--security-opt=no-new-privileges"):
            self.assertIn(option, container)
        self.assertFalse(any(c.startswith(("--volume", "--mount", "--env")) for c in container))
        self.assertNotIn(JIT, " ".join(container))
        self.assertEqual(container[container.index("--cpuset-cpus") + 1], "40-79")
        notification = ["systemd-notify", "RESTART_RESET=1"]
        self.assertIn(notification, commands)
        self.assertLess(commands.index(notification), commands.index(container))

    def test_slot_units_keep_restarting_with_backoff(self):
        for profile, prefix in (("tl3", "tl3-ci-runner"),
                                ("sandbox", "tl3-ci-runner-sandbox")):
            with self.subTest(profile=profile):
                result = self.run_script("install.sh", "0", profile)
                self.assertEqual(result.returncode, 0, result.stderr)
                unit = configparser.ConfigParser(interpolation=None)
                unit.read(self.home / f".config/systemd/user/{prefix}@.service")
                self.assertEqual(unit["Unit"].get("StartLimitIntervalSec"), "0",
                                 "successful job exits must never exhaust the start burst")
                self.assertGreater(int(unit["Service"]["RestartSteps"]), 0)
                self.assertEqual(unit["Service"]["RestartMaxDelaySec"], "5min")
                self.assertEqual(unit["Service"]["NotifyAccess"], "all")

    def test_listener_receives_jit_in_environment(self):
        listener = self.bin / "listener"
        listener.write_text("#!/usr/bin/env python3\n" +
                            "import json, os, sys\n" +
                            "print(json.dumps([sys.argv[1:], " +
                            "os.environ.get('ACTIONS_RUNNER_INPUT_JITCONFIG')]))\n")
        listener.chmod(0o700)
        entrypoint = self.home / "entrypoint.sh"
        entrypoint.write_text((ROOT / "runner-entrypoint.sh").read_text().replace(
            "/home/runner/bin/Runner.Listener", str(listener)))
        result = subprocess.run(["bash", str(entrypoint)], env=self.env,
                                input=f"{JIT}\n", text=True, capture_output=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout), [["run"], JIT],
                         "the shared listener must receive JIT input through its environment")

    def test_default_slot_keeps_tl3_routing(self):
        self.assert_slot("tl3-ci-runner", "trustladderiii", "tl3-fleet", "tl3-ci-runner")

    def test_sandbox_slot_registers_only_for_sandbox(self):
        self.assert_slot("tl3-ci-runner-sandbox", "tl3-sandbox", "tl3-sandbox-fleet",
                         "tl3-ci-runner-sandbox")

    def test_both_profiles_reset_backoff_after_ten_quick_exits(self):
        for profile in ("tl3-ci-runner", "tl3-ci-runner-sandbox"):
            with self.subTest(profile=profile):
                self.calls.unlink(missing_ok=True)
                for _ in range(10):
                    result = self.run_script("runner-slot.sh", "1", profile)
                    self.assertEqual(result.returncode, 0, result.stderr)
                commands = self.commands()
                self.assertEqual(len(commands), 30)
                for i in range(10):
                    self.assertEqual(commands[i * 3][0], "gh")
                    self.assertEqual(commands[i * 3 + 1], ["systemd-notify", "RESTART_RESET=1"])
                    self.assertEqual(commands[i * 3 + 2][:2], ["podman", "run"])

    def test_both_profiles_record_only_registration_exit_status(self):
        (self.bin / "gh").write_text("#!/bin/bash\necho sensitive-response >&2\nexit 42\n")
        for profile in ("tl3-ci-runner", "tl3-ci-runner-sandbox"):
            with self.subTest(profile=profile):
                result = self.run_script("runner-slot.sh", "1", profile)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("gh exit 42", result.stderr)
                self.assertNotIn("sensitive-response", result.stdout + result.stderr)
                self.assertFalse(self.calls.exists(), "failed registration must retain backoff")

    def test_both_profiles_preserve_explicit_cpu_placement(self):
        for profile in ("tl3-ci-runner", "tl3-ci-runner-sandbox"):
            with self.subTest(profile=profile):
                self.calls.unlink(missing_ok=True)
                self.env["CI_RUNNER_CPUSET"] = "44-47"
                result = self.run_script("runner-slot.sh", "1", profile)
                self.assertEqual(result.returncode, 0, result.stderr)
                container = next(c for c in self.commands() if c[:2] == ["podman", "run"])
                self.assertEqual(container[container.index("--cpuset-cpus") + 1], "44-47")

    def test_install_defaults_match_each_live_unit(self):
        for profile, prefix, filename in (("tl3", "tl3-ci-runner", "runner.env"),
                                          ("sandbox", "tl3-ci-runner-sandbox",
                                           "tl3-ci-runner-sandbox.env")):
            with self.subTest(profile=profile):
                result = self.run_script("install.sh", "0", profile)
                self.assertEqual(result.returncode, 0, result.stderr)
                config = self.home / f".config/tl3-ci-runners/{filename}"
                self.assertEqual(config.read_text(), f"CI_RUNNER_IMAGE={IMAGE}\nCI_RUNNER_CPUSET=40-79\n")
                unit = configparser.ConfigParser(interpolation=None)
                unit.read(self.home / f".config/systemd/user/{prefix}@.service")
                environment_file = unit["Service"]["EnvironmentFile"]
                self.assertEqual(environment_file.replace("%p", prefix),
                                 f"%h/.config/tl3-ci-runners/{filename}")
                expected_args = "%i %p" if profile == "sandbox" else "%i"
                self.assertEqual(unit["Service"]["ExecStart"],
                                 f"%h/.local/lib/tl3-ci-runners/runner-slot.sh {expected_args}")

    def test_sandbox_rejects_third_slot_before_registration(self):
        result = self.run_script("runner-slot.sh", "3", "tl3-ci-runner-sandbox")
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(self.calls.exists())

    def test_rebuild_updates_every_profile_before_removing_unreferenced_images(self):
        new_image = "sha256:" + "b" * 64
        other_image = "sha256:" + "c" * 64
        self.env.update(OLD_IMAGE=IMAGE, NEW_IMAGE=new_image)
        config = self.home / ".config/tl3-ci-runners"
        config.mkdir(parents=True)
        for profile in ("tl3", "sandbox"):
            with self.subTest(profile=profile):
                self.calls.unlink(missing_ok=True)
                (self.home / "built").unlink(missing_ok=True)
                (config / "runner.env").write_text(f"CI_RUNNER_IMAGE={IMAGE}\nCI_RUNNER_CPUSET=2,4\n")
                (config / "tl3-ci-runner-sandbox.env").write_text(
                    f"CI_RUNNER_IMAGE={other_image}\nCI_RUNNER_CPUSET=\n")
                result = self.run_script("install.sh", "0", profile)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual((config / "runner.env").read_text(),
                                 f"CI_RUNNER_IMAGE={new_image}\nCI_RUNNER_CPUSET=2,4\n")
                self.assertEqual((config / "tl3-ci-runner-sandbox.env").read_text(),
                                 f"CI_RUNNER_IMAGE={new_image}\nCI_RUNNER_CPUSET=\n")
                removals = [c for c in self.commands() if c[:3] == ["podman", "image", "rm"]]
                self.assertCountEqual(removals, [["podman", "image", "rm", IMAGE],
                                                  ["podman", "image", "rm", other_image]])
                self.calls.unlink()
                result = self.run_script("install.sh", "0", profile)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertFalse(any(c[:3] == ["podman", "image", "rm"] for c in self.commands()))

    def test_sandbox_install_is_idempotent_and_separate(self):
        config = self.home / ".config/tl3-ci-runners"
        config.mkdir(parents=True)
        original = f"CI_RUNNER_IMAGE={IMAGE}\nCI_RUNNER_CPUSET=2,4\n"
        (config / "runner.env").write_text(original)
        for _ in range(2):
            result = self.run_script("install.sh", "2", "sandbox")
            self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((config / "runner.env").read_text(), original)
        unit = self.home / ".config/systemd/user/tl3-ci-runner-sandbox@.service"
        self.assertTrue(unit.exists())
        text = unit.read_text()
        self.assertIn("%p-%i", text)
        self.assertIn("runner-slot.sh %i %p", text)
        self.assertIn("/%p.env", text)
        self.assertFalse((unit.parent / "tl3-ci-runner@.service").exists())
        calls = [c for c in self.commands() if c[0] == "systemctl"]
        starts = [c[-1] for c in calls if "enable" in c]
        self.assertEqual(starts, [f"tl3-ci-runner-sandbox@{i}.service" for i in (1, 2, 1, 2)])
        self.assertFalse(any("restart" in c for c in calls))
        env = config / "tl3-ci-runner-sandbox.env"
        env.write_text(f"CI_RUNNER_IMAGE={IMAGE}\nCI_RUNNER_CPUSET=6,8\n")
        result = self.run_script("install.sh", "0", "sandbox")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("CI_RUNNER_CPUSET=6,8", env.read_text())


if __name__ == "__main__":
    unittest.main()
