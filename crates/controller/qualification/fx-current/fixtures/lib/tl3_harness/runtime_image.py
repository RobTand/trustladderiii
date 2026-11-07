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


def prepare_runtime_profile(destination, *, host_interpreter, container_interpreter, podman, systemctl, mode, image_map, profile,
                            host, platform, environment, producer_commit, prerequisites, timeout=10):
    """Emit raw PrepareBundleInput for Rust's sole manifest/preflight publisher.

    Staging runs on the admitted named host. No image mutation, RuntimeIdentity,
    final manifest or final publication is implemented in Python.
    """
    from .runtime import stage_host_runtime

    observations = preflight_profile(image_map, profile, host=host, platform=platform, podman=podman,
                                     environment=environment, timeout=timeout)
    root = Path(destination).resolve()
    root.mkdir(mode=0o700)
    executable, inventory, trusted = stage_host_runtime(
        root / 'code', interpreter=host_interpreter, producer_commit=producer_commit,
        podman=podman, systemctl=systemctl)

    def write(path, value):
        with path.open('x') as target:
            path.chmod(0o600)
            json.dump(value, target, sort_keys=True)
            target.write('\n')
            target.flush()
            os.fsync(target.fileno())

    inspection = root / 'image-inspect.json'
    write(inspection, observations['store'])
    source_root = executable.parent
    command = {'op': 'prepare_runtime_bundle', 'input': {
        'source_root': str(source_root), 'code_inventory': str(inventory.relative_to(source_root)),
        'collector': str(executable.relative_to(source_root)), 'cleanup': 'host-cleanup',
        'trusted_root': str(trusted.relative_to(source_root)), 'producer_commit': producer_commit,
        'container_interpreter': container_interpreter, 'cleanup_arguments': [],
        'image_inventory': observations['map']['path'], 'image_inspect': str(inspection),
        'profile': profile, 'mode': mode, 'prerequisites': prerequisites,
    }}
    entry = root / 'prepare-bundle-command.json'
    write(entry, command)
    # Provenance is an observation outside source_root, never an unlisted import.
    write(root / 'image-provenance.json', observations['map'])
    return entry


def producer_main():
    import argparse

    parser = argparse.ArgumentParser(description='Stage raw inputs for the Rust runtime bundle producer')
    for option in ('destination', 'host-interpreter', 'container-interpreter', 'podman', 'systemctl', 'mode',
                   'image-map', 'profile', 'host', 'platform', 'producer-commit', 'mode-prerequisites'):
        parser.add_argument('--' + option, required=True)
    args = parser.parse_args()
    with Path(args.mode_prerequisites).open('rb') as source:
        raw = source.read(65537)
    if len(raw) > 65536:
        raise ValueError('mode prerequisite observations exceed byte bound')
    entry = prepare_runtime_profile(
        args.destination, host_interpreter=args.host_interpreter, container_interpreter=args.container_interpreter,
        podman=args.podman, systemctl=args.systemctl, mode=args.mode, image_map=args.image_map,
        profile=args.profile, host=args.host, platform=args.platform, producer_commit=args.producer_commit,
        prerequisites=json.loads(raw),
        environment={'HOME': '/home/rob', 'XDG_RUNTIME_DIR': '/run/user/1000', 'PATH': '/usr/bin:/bin'})
    print(entry)


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
