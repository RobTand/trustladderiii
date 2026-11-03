# Disk scratch for served repositories

Rob decided #639 on 2026-11-03:

> Use a plain bind of a private child directory inside the action's PB-owned
> scratch-lifetime-v1 leaf, mounted at /tmp for pytest. That replaces the tmpfs.
> Rob accepts that inherited mount flags may allow exec, suid and dev on that scratch.

The harness creates a fresh private child of the registered ephemeral leaf for
each container invocation. Only that child is bind-mounted at `/tmp`. Maker
test runs, rehearsal and Checking use the same `container_command` path. The
controller and worker submissions seal the scratch selection and accounting.

The root is `/home/rob/tmp`. The declared scratch demand is 512 MiB, rounded
up to 1 GiB of spool capacity by PrismaBuild. This is admission accounting,
not a filesystem quota. The old 512 MiB tmpfs limit no longer applies. There
was no separate tmpfs memory surcharge in TL3's submissions to remove; the
ordinary action memory demands remain unchanged. Disk files no longer occupy
a dedicated tmpfs, though filesystem caches can still consume action memory.

PrismaBuild registers the leaf before starting the action. The harness checks
its registered device/inode identity, private permissions and owner before
creating a child. Cleanup uses the existing sequence:
`pool.cleanup_action_containers`, then `scratch_lifetime.finalize`. It does
not depend on a Python finally block. Success, setup failure, cancellation
and watchdog termination all retain the same PB cleanup owner.

Under Rob's #639 policy call, the lead also decided:

> Run the pytest container as the PB worker's own UID:GID, read from the
> scratch leaf's owner, instead of 65534.

This lets PB remove test-created mode-0700 directories without privileged
cleanup or UID mapping. The harness reads both IDs from the registered leaf;
it does not assume UID 1000. A qualification with UID 65534 demonstrated why
the old identity could prevent the leaf owner from recursively cleaning it.

The threat model remains that served Python test code can already execute
arbitrary instructions in the container. Executable scratch allows copied
binaries, generated scripts and native libraries to run there too. They get
the same access as test code and can interfere with other tests in that
container. The previous `/tmp` noexec flag was never a Python-code execution
boundary. Checkout and harness mounts use `type=bind,...,readonly`, with no
explicit noexec flag, and the image itself contains executable Python.

The new `/tmp` bind has no explicit noexec, nosuid or nodev options. It inherits
the host filesystem's mount flags. This is Rob's accepted containment-policy
change, not a promise that the old tmpfs flags survive. `exec_scratch` is
retired from configuration and action inputs because it no longer controls
anything. Existing configurations must remove it.

The checkout and trusted harness remain read-only exports. The container keeps
`--cap-drop=ALL`, `--security-opt=no-new-privileges`, `--read-only`,
`--network=none`, `--ipc=private`, `--pids-limit=128` and PB-assigned CPU
affinity. The only new host path visible is the private scratch child. No
Docker socket, fleet shared mount or GPU access is added. The trusted outer
process holds admission credentials and journals results outside the test
container. Reporting remains in-process with repository code, as before;
read-only harness files do not isolate that interpreter's runtime state.

Per-test temporary directories are removed after teardown on a best-effort
basis without changing test outcomes on cleanup errors. Session fixtures and
any residue remain under the action leaf until PB finalizes it. No daemon
volume, new PB capability, container init or harness image is required.
