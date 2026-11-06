"""Admitted image transfer I/O; the shared Rust consumer validates identity/architecture."""
import os
import hashlib
import json
from pathlib import Path
import re
import signal
import subprocess


# Inspect only metadata needed for identity. Never export image environment into logs.
INSPECT = '{"Id":{{json .ID}},"Os":{{json .Os}},"Architecture":{{json .Architecture}},"RepoDigests":{{json .RepoDigests}}}'


def resolve_profile(map_path, profile, *, host, platform):
    """Resolve the ops inventory, without copying its pins into runtime config."""
    path = Path(map_path).resolve(strict=True)
    with path.open('rb') as source:
        raw = source.read(4 * 1024 * 1024 + 1)
    if len(raw) > 4 * 1024 * 1024:
        raise ValueError('image map exceeds byte bound')
    entries = [entry for entry in json.loads(raw)['profiles'] if entry['profile'] == profile]
    if len(entries) != 1:
        raise ValueError('image profile must have one ops map entry')
    entry = entries[0]
    for field in ('manifest_digest', 'config_digest'):
        if not re.fullmatch(r'sha256:[0-9a-f]{64}', entry[field]):
            raise ValueError('image map requires immutable manifest and config digests')
    repository, separator, digest = entry['registry_ref'].partition('@')
    if (not separator or not repository or digest != entry['manifest_digest']
            or any(char.isspace() for char in repository)):
        raise ValueError('registry reference does not bind the mapped manifest')
    if platform != entry['platform'] or host not in entry['rootless_present_hosts']:
        raise ValueError('image profile is not provisioned for requested host/platform')
    return {'profile': entry, 'map': {'path': str(path), 'sha256': hashlib.sha256(raw).hexdigest(), 'bytes': len(raw)}}


def preflight_profile(map_path, profile, *, host, platform, podman, environment, timeout):
    """Read-only store preflight. Provisioning and placement remain external owners."""
    from .runtime import check_rootless_directory

    resolved = resolve_profile(map_path, profile, host=host, platform=platform)
    check_rootless_directory(environment)
    entry = resolved['profile']
    reference = entry['registry_ref']
    subprocess.run([podman, 'image', 'exists', reference], env=environment, stdin=subprocess.DEVNULL,
                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=timeout, check=True)
    inspected = subprocess.run([podman, 'image', 'inspect', '--format', INSPECT, reference], env=environment,
                               stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                               timeout=timeout, check=True)
    metadata = json.loads(inspected.stdout)
    if (metadata['Id'].removeprefix('sha256:') != entry['config_digest'].removeprefix('sha256:')
            or f"{metadata['Os']}/{metadata['Architecture']}" != entry['platform']):
        raise ValueError('actual rootless image identity/platform differs from ops map')
    resolved['store'] = metadata
    return resolved


def prepare_runtime_profile(destination, *, host_interpreter, container_interpreter, podman, systemctl,
                            mode, memory_limit_bytes, image_map, profile, host, platform, environment,
                            producer_commit, mode_observations=None, timeout=10):
    """Prepare artifact inputs for Rust's generated RuntimeIdentity, not a mirror.

    Execute only on the admitted named host, with ops-provided executable paths.
    Image observations are read-only. Missing mode observations confer no runtime
    capability; Rust must refuse qualification until its evidence is supplied.
    """
    from .runtime import stage_host_runtime

    for executable in (host_interpreter, container_interpreter, podman, systemctl):
        if not Path(executable).is_absolute():
            raise ValueError('prepared profile executable paths must be explicit and absolute')
    observations = preflight_profile(image_map, profile, host=host, platform=platform, podman=podman,
                                     environment=environment, timeout=timeout)
    mode_reference = None
    if mode_observations is not None:
        path = Path(mode_observations).resolve(strict=True)
        with path.open('rb') as source:
            raw = source.read(4 * 1024 * 1024 + 1)
        if len(raw) > 4 * 1024 * 1024:
            raise ValueError('mode observations exceed byte bound')
        mode_reference = {'path': str(path), 'sha256': hashlib.sha256(raw).hexdigest(), 'bytes': len(raw)}
    prepared = {'host_interpreter': host_interpreter, 'container_interpreter': container_interpreter,
                'podman': podman, 'systemctl': systemctl, 'container_mode': mode,
                'memory_limit_bytes': memory_limit_bytes, 'scratch_root': '/var/tmp/flux-vanilla',
                'host': host, 'platform': platform,
                'access_environment': {key: environment[key] for key in ('HOME', 'XDG_RUNTIME_DIR')},
                'image_observations': observations, 'mode_observations': mode_reference}
    executable, _, _ = stage_host_runtime(destination, interpreter=host_interpreter, producer_commit=producer_commit,
                                         _prepared_profile=prepared)
    return executable.parent / 'prepared-profile.json'


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
