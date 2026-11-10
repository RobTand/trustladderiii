"""Pinned PQ dependencies and the P-8 plan-to-pytest contract for admitted jobs.

The selector itself runs separately with base/head Git history and quarantine
ownership access. This consumer never substitutes a handwritten file mapping.
"""
import argparse
import hashlib
import importlib.util
import json
from pathlib import Path, PurePosixPath
import re


COMMIT = '2c0c028d2bd87ae1665e339c9c6ec07e7fc7df32'
MARKERS = 'not cuda and not fleet_data and not task_suite and not own_cgroup and not slow'
BASE = r'(?:docker.io/library/)?python:3\.12-slim-bookworm@sha256:[0-9a-f]{64}'


def base_reference(value):
    if not re.fullmatch(BASE, value):
        raise ValueError('requires a digest-pinned Python 3.12 base')
    return value


def load_inputs(images):
    manifest = json.loads((images / 'prismaquant-inputs.json').read_bytes())
    raw = (images / 'prismaquant.txt').read_bytes()
    digest = hashlib.sha256(raw).hexdigest()
    if (manifest['commit'] != COMMIT or digest != manifest['requirements_sha256']
            or manifest['files'] != {'prismaquant.txt': digest}):
        raise ValueError('PQ requirements input hash or commit mismatch')
    if b'prismabuild' in raw.lower() or b'pbsdk' in raw.lower():
        raise ValueError('PQ image must omit the retired executor SDK')
    return manifest, {'requirements_linux_x86_64.txt': raw}


def stage_inputs(stage, inputs):
    raw = inputs['requirements_linux_x86_64.txt']
    (stage / 'requirements_linux_x86_64.txt').write_bytes(raw)
    blocks = re.split(rb'(?m)(?=^[A-Za-z0-9][A-Za-z0-9_.-]*(?:==| @ ))', raw)
    bootstrap = [block for block in blocks if block.startswith(b'setuptools==')]
    if len(bootstrap) != 1:
        raise ValueError('PQ lock must contain exactly one setuptools bootstrap pin')
    (stage / 'bootstrap.txt').write_bytes(bootstrap[0])


def install_script(snapshot='20261110T000000Z'):
    if not re.fullmatch(r'[0-9]{8}T[0-9]{6}Z', snapshot):
        raise ValueError('invalid Debian snapshot timestamp')
    return r'''
set -eu
export DEBIAN_FRONTEND=noninteractive PIP_CONFIG_FILE=/dev/null PIP_DISABLE_PIP_VERSION_CHECK=1
rm -f /etc/apt/sources.list.d/debian.sources
cat > /etc/apt/sources.list <<'APT'
deb [check-valid-until=no] https://snapshot.debian.org/archive/debian/SNAPSHOT/ bookworm main
deb [check-valid-until=no] https://snapshot.debian.org/archive/debian/SNAPSHOT/ bookworm-updates main
deb [check-valid-until=no] https://snapshot.debian.org/archive/debian-security/SNAPSHOT/ bookworm-security main
APT
apt-get update
apt-get install --no-install-recommends -y build-essential git ca-certificates libgomp1 bubblewrap
dpkg-query -W > /receipts/system-packages.txt
apt-get clean
rm -rf /var/lib/apt/lists/*
python3 -m venv /opt/tl3
/opt/tl3/bin/python -m pip install --no-cache-dir --require-hashes --no-deps --only-binary=:all: --index-url https://pypi.org/simple --report /receipts/bootstrap-report.json -r /input/bootstrap.txt
/opt/tl3/bin/python -m pip install --no-cache-dir --require-hashes --no-deps --no-build-isolation --index-url https://pypi.org/simple --extra-index-url https://download.pytorch.org/whl/cpu --report /receipts/pip-report.json -r /input/requirements_linux_x86_64.txt
git -c core.hooksPath=/dev/null clone --quiet --no-checkout --template= https://github.com/RobTand/prismaquant.git /opt/prismaquant-source
git -C /opt/prismaquant-source -c core.hooksPath=/dev/null checkout --quiet --detach PQ_COMMIT
test "$(git -C /opt/prismaquant-source rev-parse HEAD)" = PQ_COMMIT
cmp /opt/prismaquant-source/tools/bazel/requirements_linux_x86_64.txt /input/requirements_linux_x86_64.txt
git -C /opt/prismaquant-source rev-parse HEAD > /receipts/pq-source-head.txt
cd /opt/prismaquant-source
/opt/tl3/bin/python -m pip install --no-cache-dir --no-deps --no-build-isolation -e . --report /receipts/subject-report.json
/opt/tl3/bin/python -m pip check
/opt/tl3/bin/python -c "import tessera.metadata, prismaquant"
/opt/tl3/bin/python -m pip freeze --all > /receipts/freeze.txt
# Runtime containers mount only job scratch at /tmp. These aliases expose the
# requested paths without adding a host output mount to the containment path.
ln -s /tmp/tl3-output /output
ln -s /tmp/tl3-scratch /scratch
/opt/tl3/bin/python - <<'PYTHON'
import importlib.metadata as m
import json, os, pathlib, sys
import torch
assert sys.version_info[:2] == (3, 12), sys.version
assert torch.version.cuda is None, 'CPU-only Torch required'
torch.set_num_threads(1)
installed = {d.metadata['Name']: d.version for d in m.distributions()}
assert not any(n.lower().replace('_', '-') in ('prismabuild', 'pbsdk') for n in installed)
affinity = sorted(os.sched_getaffinity(0))
assert affinity == json.loads(pathlib.Path('/input/affinity.json').read_text()), 'CPU affinity changed'
pathlib.Path('/receipts/inventory.json').write_text(json.dumps({
    'python': sys.version, 'installed': installed, 'affinity': affinity,
    'torch_cuda': torch.version.cuda, 'native_threads': torch.get_num_threads()}, sort_keys=True) + '\n')
PYTHON
'''.replace('SNAPSHOT', snapshot).replace('PQ_COMMIT', COMMIT)


def _selector(value, *, node=False):
    if not isinstance(value, str) or len(value) > 4096 or any(ord(c) < 32 for c in value):
        raise ValueError('invalid PQ test selector')
    file = value.split('::', 1)[0]
    path = PurePosixPath(file)
    if (not file.startswith('tests/') or path.suffix != '.py' or path.is_absolute()
            or file != path.as_posix() or any(p in ('.', '..', '.git') for p in path.parts)
            or any(c in file for c in '\\*?[]') or not node and '::' in value):
        raise ValueError('invalid PQ test selector')
    return value


def pytest_argv(plan, *, base, head):
    for revision in (base, head):
        if not re.fullmatch(r'[0-9a-f]{40}', revision):
            raise ValueError('requires exact base/head commits')
    if (plan['schema'] != 'prismaquant.gate_plan.v1'
            or (plan['base_sha'], plan['head_sha']) != (base, head)):
        raise ValueError('PQ selector plan identity mismatch')
    files = [_selector(row['path']) for row in plan['selected']]
    if not files:
        raise ValueError('empty selection requires an explicit no-tests decision')
    if len(set(files)) != len(files):
        raise ValueError('duplicate PQ selected file')
    deselect = [_selector(node, node=True) for node in plan['deselect']]
    if any('::' not in node or node.split('::', 1)[0] not in files for node in deselect):
        raise ValueError('PQ deselection must name a selected module node')
    if not isinstance(plan['deferred'], list):
        raise ValueError('PQ selector plan must retain deferred lanes')
    return ['python', '-B', '-m', 'pytest', *files, '-q', '-ra', '--timeout=600',
            '-n', '2', '--dist=worksteal', '-m', MARKERS,
            *['--deselect=' + node for node in deselect], '--junitxml=/output/junit.xml',
            '-o', 'cache_dir=/scratch/pytest-cache']


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--base', required=True, type=base_reference)
    parser.add_argument('--base-config-digest', required=True)
    parser.add_argument('--debian-snapshot', required=True)
    args = parser.parse_args()
    if not re.fullmatch(r'sha256:[0-9a-f]{64}', args.base_config_digest):
        raise ValueError('requires the observed base config digest')
    installer = install_script(args.debian_snapshot)
    images = Path(__file__).with_name('images')
    manifest, inputs = load_inputs(images)
    manifest.update(base_image=args.base, base_config_digest=args.base_config_digest,
                    debian_snapshot=args.debian_snapshot,
                    profile_builder_sha256=hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
                    install_script_sha256=hashlib.sha256(installer.encode()).hexdigest())
    spec = importlib.util.spec_from_file_location('tl3_image_publish', Path(__file__).with_name('flux_tessera_image.py'))
    publisher = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(publisher)

    publisher.publish_image(manifest, inputs, profile='prismaquant', installer=lambda: installer,
                            stage_inputs=stage_inputs, manifest_path=images / 'prismaquant-inputs.json')


if __name__ == '__main__':
    main()
