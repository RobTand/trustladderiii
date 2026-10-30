"""Runner boundary checks. Run through PrismaBuild, never against live credentials."""

import configparser
from pathlib import Path
import subprocess
import tempfile
import unittest


HERE = Path(__file__).resolve().parent
OLD = "sha256:" + "a" * 64
NEW = "sha256:" + "b" * 64


class Hardening(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.bin = self.root / "bin"
        self.bin.mkdir()
        self.events = self.root / "events"
        self.env = {
            "PATH": f"{self.bin}:/usr/bin:/bin",
            "HOME": str(self.root),
            "EVENTS": str(self.events),
            "CI_RUNNER_IMAGE": NEW,
        }

    def stub(self, name, body):
        path = self.bin / name
        path.write_text("#!/bin/bash\nset -eu\n" + body + "\n")
        path.chmod(0o700)
        return path

    def run_script(self, name, *args, input=None):
        return subprocess.run(
            ["bash", str(HERE / name), *args], env=self.env,
            input=input, text=True, capture_output=True, check=False,
        )

    def slot_stubs(self):
        self.stub("gh", 'printf "synthetic-jit-input\\n"')
        self.stub("podman", 'printf "podman %s\\n" "$*" >> "$EVENTS"')
        self.stub("systemd-notify", 'printf "notify %s\\n" "$*" >> "$EVENTS"')

    def test_ten_quick_exits_and_backoff_reset(self):
        unit = configparser.ConfigParser(interpolation=None)
        unit.read(HERE / "tl3-ci-runner@.service")
        self.slot_stubs()
        for _ in range(10):
            result = self.run_script("runner-slot.sh", "1")
            self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(len(self.events.read_text().splitlines()), 20,
                         "each successful registration must reset backoff before Podman")
        self.assertEqual(unit["Unit"]["StartLimitIntervalSec"], "0",
                         "normal exits must never exhaust a start burst")
        self.assertGreater(int(unit["Service"]["RestartSteps"]), 0)
        self.assertIn("RestartMaxDelaySec", unit["Service"])
        self.assertEqual(unit["Service"]["NotifyAccess"], "all")
        for notify, podman in zip(*[iter(self.events.read_text().splitlines())] * 2):
            self.assertIn("RESTART_RESET=1", notify)
            self.assertTrue(podman.startswith("podman run "))
        print("ten quick exits: 10/10; registration resets: 10/10; start limit disabled")

    def test_registration_error_records_status_only(self):
        self.slot_stubs()
        self.stub("gh", 'echo "sensitive response" >&2; exit 42')
        result = self.run_script("runner-slot.sh", "1")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("42", result.stderr)
        self.assertNotIn("sensitive response", result.stdout + result.stderr)
        self.assertFalse(self.events.exists(), "failed registration must retain backoff")

    def test_credential_handoff_keeps_argv_clean(self):
        listener = self.stub("listener", 'printf \'argv=%s\\n\' "$*"\nif [[ ${ACTIONS_RUNNER_INPUT_JITCONFIG:-} == synthetic-jit-input ]]; then\n    echo \'jit_input=set\'\nelse\n    echo \'jit_input=unset\'\nfi')
        entrypoint = self.root / "entrypoint.sh"
        entrypoint.write_text((HERE / "runner-entrypoint.sh").read_text().replace(
            "/home/runner/bin/Runner.Listener", str(listener)))
        result = subprocess.run(["bash", str(entrypoint)], env=self.env,
                                input="synthetic-jit-input\n", text=True, capture_output=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.splitlines(), ["argv=run", "jit_input=set"])

    def test_cpu_placement_and_quota(self):
        self.slot_stubs()
        result = self.run_script("runner-slot.sh", "2")
        self.assertEqual(result.returncode, 0, result.stderr)
        invocation = self.events.read_text()
        self.assertIn("--cpuset-cpus 40-79", invocation)
        self.assertIn("--cpus=4", invocation)
        self.env["CI_RUNNER_CPUSET"] = "44-47"
        result = self.run_script("runner-slot.sh", "2")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("--cpuset-cpus 44-47", self.events.read_text())

    def installer_stubs(self):
        self.stub("uname", "echo x86_64")
        self.stub("id", "echo 1000")
        self.stub("gh", "exit 0")
        self.stub("systemctl", 'printf "systemctl %s\\n" "$*" >> "$EVENTS"')
        self.stub("podman", 'printf \'podman %s\\n\' "$*" >> "$EVENTS"\ncase "$*" in\n    \'info \'*) echo true ;;\n    \'image inspect \'*)\n        if [[ -f "$HOME/built" ]]; then echo "$NEW_IMAGE"; else echo "$OLD_IMAGE"; fi ;;\n    \'build \'*)\n        [[ ${FAIL_BUILD:-0} == 0 ]] || exit 9\n        touch "$HOME/built" ;;\nesac')
        self.env.update(OLD_IMAGE=OLD, NEW_IMAGE=NEW)

    def test_image_cleanup_only_after_successful_retag(self):
        self.installer_stubs()
        result = self.run_script("install.sh", "0")
        self.assertEqual(result.returncode, 0, result.stderr)
        events = self.events.read_text()
        self.assertIn(f"podman image rm {OLD}", events)
        self.assertLess(events.index("podman build "), events.index(f"podman image rm {OLD}"))
        self.assertNotIn("prune", events)
        self.assertNotIn("--force", events)
        self.assertIn("CI_RUNNER_CPUSET=40-79", (self.root / ".config/tl3-ci-runners/runner.env").read_text())
        self.events.unlink()
        result = self.run_script("install.sh", "0")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotIn("podman image rm", self.events.read_text(), "same image must survive reinstall")

    def test_failed_build_retains_previous_image(self):
        self.installer_stubs()
        self.env["FAIL_BUILD"] = "1"
        result = self.run_script("install.sh", "0")
        self.assertEqual(result.returncode, 9)
        self.assertNotIn("podman image rm", self.events.read_text())


if __name__ == "__main__":
    unittest.main(verbosity=2)
