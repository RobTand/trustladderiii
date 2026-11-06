"""Admitted image transfer I/O; the shared Rust consumer validates identity/architecture."""
import os
from pathlib import Path
import signal
import subprocess


# Inspect only metadata needed for identity. Never export image environment into logs.
INSPECT = '{"Id":{{json .Id}},"Os":{{json .Os}},"Architecture":{{json .Architecture}},"RepoDigests":{{json .RepoDigests}}}'


def _stop(child, timeout):
    if child is None or child.returncode is not None:
        return
    try:
        os.killpg(child.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    child.wait(timeout=timeout)


def ensure_image(image, *, podman, exporter, environment, log, timeout, validate, registry=False):
    """Called only within authorized admission, with Rust-validated pins and explicit env.

    validate receives allowlisted Podman metadata and raises on identity/platform
    mismatch. It must call the generated Rust preflight, never duplicate its rules.
    Existing valid images need no export; failed imports can be retried by identity.
    registry selects an already approved exact manifest reference. Execution is
    ops-owned; this helper performs no admission or host configuration changes.
    """
    from .runtime import check_rootless_directory

    check_rootless_directory(environment)
    exists = subprocess.run([podman, 'image', 'exists', image], env=environment,
                            stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                            stderr=subprocess.DEVNULL, timeout=timeout, check=False)
    if exists.returncode not in (0, 1):
        raise subprocess.CalledProcessError(exists.returncode, exists.args)
    if exists.returncode == 1:
        saved = loaded = None
        with Path(log).open('ab') as journal:
            try:
                if registry:
                    loaded = subprocess.Popen([podman, 'pull', '--tls-verify=false', image], env=environment,
                                              stdin=subprocess.DEVNULL, stdout=journal, stderr=journal,
                                              start_new_session=True)
                    code = loaded.wait(timeout=timeout)
                    if code:
                        raise subprocess.CalledProcessError(code, loaded.args)
                else:
                    saved = subprocess.Popen([exporter, 'save', image], env=environment,
                                             stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                                             stderr=journal, start_new_session=True)
                    loaded = subprocess.Popen([podman, 'load'], env=environment, stdin=saved.stdout,
                                              stdout=journal, stderr=journal, start_new_session=True)
                    saved.stdout.close()
                    load_code = loaded.wait(timeout=timeout)
                    save_code = saved.wait(timeout=timeout)
                    if load_code or save_code:
                        raise RuntimeError(f'image transfer incomplete: export={save_code}, load={load_code}')
            finally:
                cleanup_error = None
                for child in (loaded, saved):
                    try:
                        _stop(child, timeout)
                    except (OSError, subprocess.TimeoutExpired) as error:
                        cleanup_error = error
                if saved is not None and saved.stdout is not None:
                    saved.stdout.close()
                journal.flush()
                os.fsync(journal.fileno())
                if cleanup_error is not None:
                    raise cleanup_error
    inspected = subprocess.run([podman, 'image', 'inspect', '--format', INSPECT, image], env=environment,
                               stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                               timeout=timeout, check=True)
    validate(inspected.stdout)
    return inspected.stdout
