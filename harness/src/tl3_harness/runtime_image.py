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


def ensure_image(image, *, podman, exporter, environment, log, timeout, validate):
    """Called only within authorized admission, with Rust-validated pins and explicit env.

    validate receives allowlisted Podman metadata and raises on identity/platform
    mismatch. It must call the generated Rust preflight, never duplicate its rules.
    Existing valid images need no export; failed imports can be retried by identity.
    This helper performs no admission, registry pull or host provisioning itself.
    """
    exists = subprocess.run([podman, 'image', 'exists', image], env=environment,
                            stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                            stderr=subprocess.DEVNULL, timeout=timeout, check=False)
    if exists.returncode not in (0, 1):
        raise subprocess.CalledProcessError(exists.returncode, exists.args)
    if exists.returncode == 1:
        saved = loaded = None
        with Path(log).open('ab') as journal:
            try:
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
                _stop(loaded, timeout)
                _stop(saved, timeout)
                journal.flush()
                os.fsync(journal.fileno())
    inspected = subprocess.run([podman, 'image', 'inspect', '--format', INSPECT, image], env=environment,
                               stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                               timeout=timeout, check=True)
    validate(inspected.stdout)
    return inspected.stdout
