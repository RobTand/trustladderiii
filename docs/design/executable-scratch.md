# Executable scratch for served repositories

Ticket #236 originally proposed Git metadata, executable scratch and disk
scratch. This change implements executable scratch only. Git metadata and
scratch backed by a host filesystem remain outside the harness boundary.

A repository may set `exec_scratch = true` in its `[[github.repos]]` entry.
Omission means false. The controller selects this option by repository owner,
name and installation, for both check plans and maker test runs. The model
cannot change it through test selectors. Base-result cache identity includes
the option so a restricted result cannot stand in for an executable run.

The opt-in changes the private `/tmp` tmpfs mount from noexec to exec. The
runner still points TMPDIR, TEMP, TMP and Python's tempfile module at each
test's pytest temporary directory. It removes that directory after teardown,
including failed tests, without changing the test's outcome on cleanup errors.
Session-scoped temporary factories can retain data until the container exits.
The shared 512 MiB mount bounds all scratch, including retained session data
and files whose cleanup failed. Container removal discards the mount.

Executable scratch allows copied executables, generated scripts and native
libraries to run or load there. A compiled program receives the same access
as Python test code. It can consume the action's CPU, memory and scratch and
can interfere with other tests in that container. The mount has nosuid and
nodev; it grants neither a privilege transition nor access to host devices.
The tmpfs consumes RAM charged to the admitted action's memory slice. Its
512 MiB ceiling does not reserve additional memory beyond the action demand;
tests must fit their remaining memory within that demand too.

The checkout and trusted harness remain read-only exports. The container still
runs as UID/GID 65534, with no network, all capabilities dropped,
no-new-privileges, private IPC, a PID bound and PB-assigned CPU affinity. There
is no writable host mount, Docker socket, fleet shared mount or GPU access.
The trusted outer process journals results and holds PB admission credentials
outside the test container. Reporting remains in-process, as before; this
option does not change that accepted custody limit.

An image pin selects dependencies; it does not opt a repository into execution
on scratch. Turning the option off restores noexec for subsequent actions.
Existing admitted actions keep their sealed setting. No live config is changed
by this implementation.
