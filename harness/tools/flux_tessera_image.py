"""Prepare and publish Tessera dependencies inside one captain fluxrun job."""
import hashlib
import json
import os
from pathlib import Path
import re
import socket
import subprocess
import tempfile
import urllib.request


REPOSITORY = 'dl380g10:5000/tl3/tessera'
DIGEST = r'sha256:[0-9a-f]{64}'
INSPECT = '{"Id":{{json .ID}},"Os":{{json .Os}},"Architecture":{{json .Architecture}}}'
THREADS = ('OMP_NUM_THREADS', 'MKL_NUM_THREADS', 'OPENBLAS_NUM_THREADS',
           'NUMEXPR_NUM_THREADS', 'TORCH_NUM_THREADS', 'RAYON_NUM_THREADS',
           'GOMAXPROCS', 'CARGO_BUILD_JOBS')


def sha(raw):
    return hashlib.sha256(raw).hexdigest()


def load_inputs(images):
    manifest = json.loads((images / 'tessera-inputs.json').read_bytes())
    inputs = {}
    for name, expected in manifest['files'].items():
        path = Path(name)
        if path.is_absolute() or '..' in path.parts:
            raise ValueError('invalid image input path')
        raw = (images / path).read_bytes()
        if sha(raw) != expected:
            raise ValueError('input hash mismatch: ' + name)
        inputs[name] = raw
    if sha(inputs['tessera.txt']) != manifest['requirements_sha256']:
        raise ValueError('requirements hash mismatch')
    lock = inputs['tessera-gate/resolved-requirements.txt'].decode()
    sources = b''.join(inputs['tessera-gate/' + name] for name in (
        'gate_requirements.txt', 'gate_torch_requirements.txt', 'gate_prismaquant_requirements.txt'))
    if '# source-sha256: ' + sha(sources) + '\n' not in lock:
        raise ValueError('resolved lock does not bind gate sources')
    pins = []
    for line in lock.splitlines():
        if re.match(r'^[A-Za-z0-9][A-Za-z0-9_.-]*(?:==| @ )', line):
            if "sys_platform == 'win32'" not in line:
                pins.append(line.split(';')[0].rstrip(' \\'))
    expected = '\n'.join(sorted(pins + manifest['image_tooling_pins'], key=str.lower)) + '\n'
    if expected.encode() != inputs['tessera.txt']:
        raise ValueError('Tessera pins differ from landed gate resolution')
    if 'prismabuild' in expected.lower() or 'tessera-quant' in expected.lower():
        raise ValueError('image must omit executor SDK and subject package')
    if not re.fullmatch(REPOSITORY + '@' + DIGEST, manifest['base_image']):
        raise ValueError('base image must be pinned by registry manifest digest')
    return manifest, inputs


def install_script():
    return r'''
set -eu
export PIP_CONFIG_FILE=/dev/null PIP_DISABLE_PIP_VERSION_CHECK=1
rm -rf /opt/tl3
python3 -m venv /opt/tl3
/opt/tl3/bin/python -m pip install --no-cache-dir --no-deps --index-url https://pypi.org/simple --report /receipts/bootstrap-report.json pip==25.1.1 setuptools==81.0.0 wheel==0.48.0 packaging==26.3
/opt/tl3/bin/python -m pip install --no-cache-dir --no-deps --require-hashes --index-url https://download.pytorch.org/whl/cpu --report /receipts/torch-report.json -r /input/torch.txt
/opt/tl3/bin/python -m pip install --no-cache-dir --no-deps --no-build-isolation --require-hashes --index-url https://pypi.org/simple --report /receipts/pip-report.json -r /input/packages.txt
/opt/tl3/bin/python -m pip check
/opt/tl3/bin/python -m pip freeze --all > /receipts/freeze.txt
/opt/tl3/bin/python - <<'PY'
import importlib.metadata as m
import json, os, pathlib, sys
import torch
assert sys.version_info[:2] == (3, 14), sys.version
assert torch.version.cuda is None, 'CPU-only Torch required'
torch.set_num_threads(1)
installed = {d.metadata['Name']: d.version for d in m.distributions()}
assert not any(n.lower().replace('_', '-') in ('prismabuild', 'tessera-quant') for n in installed)
affinity = sorted(os.sched_getaffinity(0))
assert affinity == json.loads(pathlib.Path('/input/affinity.json').read_text()), 'CPU affinity changed'
pathlib.Path('/receipts/inventory.json').write_text(json.dumps({
    'python': sys.version, 'installed': installed, 'affinity': affinity,
    'torch_cuda': torch.version.cuda, 'native_threads': torch.get_num_threads()}, sort_keys=True) + '\n')
PY
'''


def registry_read(suffix):
    request = urllib.request.Request('http://' + REPOSITORY.split('/')[0] + '/v2/tl3/tessera/' + suffix,
                                    headers={'Accept': 'application/vnd.oci.image.manifest.v1+json'})
    with urllib.request.urlopen(request, timeout=60) as response:
        return response.read(4 * 1024 * 1024 + 1)


def verify_registry(digest, config_digest):
    if not re.fullmatch(DIGEST, digest) or not re.fullmatch(DIGEST, config_digest):
        raise ValueError('invalid registry digest')
    raw = registry_read('manifests/' + digest)
    if 'sha256:' + sha(raw) != digest:
        raise ValueError('registry manifest digest mismatch')
    manifest = json.loads(raw)
    if manifest['config']['digest'] != config_digest:
        raise ValueError('registry manifest names a different config')
    raw = registry_read('blobs/' + config_digest)
    if 'sha256:' + sha(raw) != config_digest:
        raise ValueError('registry config digest mismatch')
    config = json.loads(raw)
    if (config['os'], config['architecture']) != ('linux', 'amd64'):
        raise ValueError('registry image is not linux/amd64')
    return {'manifest_digest': digest, 'config_digest': config_digest, 'platform': 'linux/amd64'}


def main():
    manifest, inputs = load_inputs(Path(__file__).with_name('images'))
    job = os.environ.get('FLUX_JOB_ID', '')
    scratch = Path(os.environ.get('TMPDIR', '/missing'))
    if (os.geteuid() == 0 or socket.gethostname().split('.')[0] != 'dl380g10'
            or not re.fullmatch(r'[A-Za-z0-9]+', job)
            or os.environ.get('FLUX_F58_FORCE_ASCII') != '1'
            or scratch != Path('/var/tmp/flux-vanilla') / job or not scratch.is_dir()):
        raise ValueError('requires a rootless captain fluxrun job on dl380g10 with ASCII job id')
    out = Path(os.environ['OUT']).resolve()
    if not out.is_relative_to('/mnt/shared') or not out.is_dir():
        raise ValueError('requires a unique shared output directory from captain fluxrun')
    env = {name: os.environ[name] for name in ('PATH', 'HOME', 'XDG_RUNTIME_DIR', 'TMPDIR')}
    env.update({name: '1' for name in THREADS})

    def run(command):
        return subprocess.check_output(command, env=env, stdin=subprocess.DEVNULL).decode().strip()

    if run(['podman', 'info', '--format', '{{.Host.Security.Rootless}}']) != 'true':
        raise ValueError('Podman must be rootless')
    base = json.loads(run(['podman', 'image', 'inspect', '--format', INSPECT, manifest['base_image']]))
    if ('sha256:' + base['Id'].removeprefix('sha256:') != manifest['base_config_digest']
            or (base['Os'], base['Architecture']) != ('linux', 'amd64')):
        raise ValueError('local base config/platform mismatch')
    affinity = sorted(os.sched_getaffinity(0))
    name = 'tl3-tessera-' + job
    tag = REPOSITORY + ':flux-' + job + '-' + manifest['requirements_sha256'][:12]
    with tempfile.TemporaryDirectory(prefix='tessera-image-', dir=scratch) as directory:
        stage = Path(directory)
        # Keep each hashed requirement and its continuation lines together.
        lock = inputs['tessera-gate/resolved-requirements.txt'].decode()
        blocks = re.split(r'(?m)(?=^[A-Za-z0-9][A-Za-z0-9_.-]*(?:==| @ ))', lock)
        torch = [block for block in blocks if block.startswith('torch==')]
        packages = [block for block in blocks if not block.startswith('torch==')]
        if len(torch) != 1:
            raise ValueError('resolved lock must contain one CPU Torch pin')
        (stage / 'torch.txt').write_text(''.join(torch))
        (stage / 'packages.txt').write_text(''.join(packages))
        (stage / 'affinity.json').write_text(json.dumps(affinity))
        try:
            command = ['podman', 'run', '--pull=never', '--cgroups=disabled', '--name', name,
                       '--cpuset-cpus=' + ','.join(map(str, affinity)),
                       '--volume', str(stage) + ':/input:ro', '--volume', str(out) + ':/receipts:rw']
            for variable in THREADS:
                command += ['--env', variable + '=1']
            command += ['--entrypoint', '/bin/sh', manifest['base_image'], '-ec', install_script()]
            subprocess.run(command, env=env, stdin=subprocess.DEVNULL, check=True)
            config_digest = run(['podman', 'commit', '--format', 'oci', '--change',
                                 'ENV PATH=/opt/tl3/bin:/usr/local/bin:/usr/bin:/bin', name, tag])
            config_digest = 'sha256:' + config_digest.removeprefix('sha256:')
            if not re.fullmatch(DIGEST, config_digest):
                raise ValueError('invalid committed config digest')
        finally:
            # A failed install must not leave a container or proceed to push.
            subprocess.run(['podman', 'rm', '--force', '--ignore', name], env=env,
                           stdin=subprocess.DEVNULL, check=True)
    push_digest = out / 'push-digest.txt'
    subprocess.run(['podman', 'push', '--tls-verify=false', '--format', 'oci',
                    '--digestfile', str(push_digest), tag, 'docker://' + tag],
                   env=env, stdin=subprocess.DEVNULL, check=True)
    digest = push_digest.read_text().strip()
    verified = verify_registry(digest, config_digest)
    # Runtime preflight requires the immutable reference in the local rootless store.
    subprocess.run(['podman', 'pull', '--tls-verify=false', REPOSITORY + '@' + digest],
                   env=env, stdin=subprocess.DEVNULL, check=True)
    artifacts = {p.name: sha(p.read_bytes()) for p in out.iterdir() if p.is_file()}
    receipt = dict(schema='tl3.tessera-image-build.v1', flux_job_id=job,
                   tl3_head=run(['git', 'rev-parse', 'HEAD']),
                   tl3_tree=run(['git', 'rev-parse', 'HEAD^{tree}']),
                   builder_sha256=sha(Path(__file__).read_bytes()),
                   inputs_manifest_sha256=sha(Path(__file__).with_name('images').joinpath('tessera-inputs.json').read_bytes()),
                   inputs=manifest, registry_ref=REPOSITORY + '@' + digest, registry_tag=tag,
                   affinity=affinity, artifacts=artifacts, **verified)
    raw = (json.dumps(receipt, sort_keys=True, indent=2) + '\n').encode()
    path = out / 'image-receipt.json'
    with path.open('xb') as stream:
        stream.write(raw)
    # Captain authenticates stdout; bind the retained receipt and its artifacts to it.
    print(json.dumps({'image_receipt_sha256': sha(raw), 'registry_ref': receipt['registry_ref'],
                      'config_digest': config_digest}), flush=True)


if __name__ == '__main__':
    main()
