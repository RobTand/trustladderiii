"""Fleet provisioning boundaries. Run through PrismaBuild."""

import configparser
import unittest

from test_runners import ROOT, RunnerTests


class FleetTests(RunnerTests):
    def test_host_names_arch_labels_and_cpu_placement(self):
        for host, arch, label in (("celestia", "x86_64", "x64"),
                                  ("sparky", "aarch64", "ARM64"),
                                  ("sparklina", "aarch64", "ARM64")):
            with self.subTest(host=host):
                self.calls.unlink(missing_ok=True)
                self.env.update(TEST_HOST=host, TEST_ARCH=arch)
                result = self.run_script("runner-slot.sh", "4")
                self.assertEqual(result.returncode, 0, result.stderr)
                registration = next(c for c in self.commands() if c[0] == "gh")
                name = next(c for c in registration if c.startswith("name="))
                self.assertTrue(name.startswith(f"name=tl3-{host}-slot-4-"), name)
                self.assertIn(f"labels[]={label}", registration)
                self.assertIn(f"labels[]=tl3-host-{host}", registration)
                container = next(c for c in self.commands() if c[:2] == ["podman", "run"])
                self.assertNotIn("--cpuset-cpus", container,
                                 "dl380g10 CPU numbers must not leak to other hosts")

    def test_arm_sandbox_and_unknown_arch_fail_before_credentials(self):
        for arch, profile in (("aarch64", "tl3-ci-runner-sandbox"),
                               ("riscv64", "tl3-ci-runner")):
            with self.subTest(arch=arch):
                self.calls.unlink(missing_ok=True)
                self.env["TEST_ARCH"] = arch
                result = self.run_script("runner-slot.sh", "1", profile)
                self.assertNotEqual(result.returncode, 0)
                self.assertFalse(any(c[0] == "gh" for c in self.commands()))

    def test_install_six_x64_and_four_arm_slots(self):
        for host, arch, image_arch, slots in (("celestia", "x86_64", "amd64", "6"),
                                              ("sparky", "aarch64", "arm64", "4")):
            with self.subTest(host=host):
                self.calls.unlink(missing_ok=True)
                self.env.update(TEST_HOST=host, TEST_ARCH=arch)
                result = self.run_script("install.sh", slots)
                self.assertEqual(result.returncode, 0, result.stderr)
                build = next(c for c in self.commands() if c[:2] == ["podman", "build"])
                self.assertIn(f"TARGETARCH={image_arch}", build)
                starts = [c[-1] for c in self.commands() if c[0] == "systemctl" and "enable" in c]
                self.assertEqual(starts, [f"tl3-ci-runner@{i}.service" for i in range(1, int(slots)+1)])
                config = self.home / ".config/tl3-ci-runners/runner.env"
                self.assertIn("CI_RUNNER_CPUSET=\n", config.read_text())
                unit = configparser.ConfigParser(interpolation=None)
                unit.read(self.home / ".config/systemd/user/tl3-ci-runner@.service")
                self.assertLess(int(unit["Service"]["CPUWeight"]), 100)
                config.unlink()

    def test_rootful_runtime_refused_before_build_or_credentials(self):
        self.env["TEST_ROOTLESS"] = "false"
        result = self.run_script("install.sh", "0")
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(any(c[:2] == ["podman", "build"] or c[0] == "gh"
                             for c in self.commands()))

    def test_both_arch_images_and_rustup_downloads_are_pinned(self):
        containerfile = (ROOT / "Containerfile").read_text()
        self.assertIn("sha256:0ce3288e48b012a83685998f683a2ae0cbd284fa722c090dda0af5443215cf30", containerfile)
        self.assertIn("sha256:2089924d7fb6a96f1d1d2aca242ad6a07315b4ec2d7d275391ee265de70e941b", containerfile)
        self.assertIn("a9c168dbea81a84671c6b7a5c1795cb0eb699020110d107ddfca427bd797e697", containerfile)
        self.assertIn("5be191e8c70a5007074254cb74d78dbfdd25cd5a61c27de7b8e5410122ec24f4", containerfile)


if __name__ == "__main__":
    unittest.main(verbosity=2)
