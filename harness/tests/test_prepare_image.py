"""Trusted image profiles must install the dependency handoff, not the subject."""
import importlib.util
import hashlib
import json
from pathlib import Path
import shlex
import shutil
import subprocess
import sys

import pytest

spec = importlib.util.spec_from_file_location('prepare_image', Path(__file__).parents[1] / 'tools/prepare_image.py')
prepare_image = importlib.util.module_from_spec(spec)
spec.loader.exec_module(prepare_image)


@pytest.mark.parametrize('profile,pytest_version,triton_version', [
    ('tessera', '8.4.2', '3.8.0'),
])
def test_repository_profile_installs_frozen_cpu_dependencies(monkeypatch, profile, pytest_version, triton_version):
    commands = []
    monkeypatch.delenv('PRISMABUILD_CONTAINER_OWNER', raising=False)
    monkeypatch.delenv('PRISMABUILD_CONTAINER_MARKER', raising=False)
    monkeypatch.setenv('FLUX_JOB_ID', 'f123')
    monkeypatch.setattr(sys, 'argv', ['prepare_image.py', 'sha256:' + '1' * 64, '--profile', profile])
    monkeypatch.setattr(subprocess, 'run', lambda command, **kwargs: commands.append(command))
    monkeypatch.setattr(subprocess, 'check_output', lambda *args, **kwargs: 'sha256:' + '2' * 64)
    try:
        prepare_image.main()
    except SystemExit as error:
        assert error.code == 0, f'{profile} must be an accepted image profile'
    command = commands[0]
    script = command[-1]
    requirements = Path(__file__).parents[1] / 'tools/images' / f'{profile}.txt'
    assert requirements.is_file(), 'keep the frozen dependency list reviewable'
    pins = requirements.read_text()
    assert 'prismabuild @' not in pins, 'retired executor must not be installed in new images'
    assert f'pytest=={pytest_version}\n' in pins
    assert f'triton=={triton_version}\n' in pins
    assert 'torch==2.11.0+cpu\n' in pins
    assert f'safetensors=={"0.6.2" if profile == "tessera" else "0.8.0"}\n' in pins
    assert '--no-build-isolation' in script and '--no-deps' in script
    assert 'https://download.pytorch.org/whl/cpu' in script
    assert 'python3.14-dev' in script and 'g++' in script and 'make' in script
    assert script.index('setuptools==80.10.2') < script.index('--no-build-isolation')
    installs = [shlex.split(line) for line in script.splitlines()
                if line.startswith('/opt/tl3/bin/python -m pip ')]
    torch_installs = [command for command in installs
                      if 'https://download.pytorch.org/whl/cpu' in command]
    assert len(torch_installs) == 1
    assert 'torch==2.11.0+cpu' in torch_installs[0], 'only torch may use the PyTorch index'
    assert '--no-deps' in torch_installs[0] and '-r' not in torch_installs[0]
    assert '--extra-index-url' not in script
    for command in installs:
        if command != torch_installs[0]:
            assert command[command.index('--index-url') + 1] == 'https://pypi.org/simple'
    assert '-e .' not in script and 'pip install .' not in script
    assert commands[-1][:3] == ['docker', 'rm', '--force']


def test_retired_profile_is_refused_before_any_container_command(monkeypatch):
    monkeypatch.setenv('FLUX_JOB_ID', 'f123')
    monkeypatch.setattr(sys, 'argv', ['prepare_image.py', 'sha256:' + '1' * 64,
                                     '--profile', 'prismabuild'])
    monkeypatch.setattr(subprocess, 'run', lambda *args, **kwargs: pytest.fail('container command'))
    with pytest.raises(SystemExit) as error:
        prepare_image.main()
    assert error.value.code == 2


def test_retired_markers_do_not_authorize_image_preparation(monkeypatch):
    monkeypatch.delenv('FLUX_JOB_ID', raising=False)
    monkeypatch.setenv('PRISMABUILD_CONTAINER_OWNER', 'fixture')
    monkeypatch.setenv('PRISMABUILD_CONTAINER_MARKER', 'fixture')
    monkeypatch.setattr(sys, 'argv', ['prepare_image.py', 'sha256:' + '1' * 64])
    monkeypatch.setattr(subprocess, 'run', lambda *args, **kwargs: pytest.fail('container command'))
    with pytest.raises(RuntimeError, match='Flux job'):
        prepare_image.main()


def flux_builder():
    path = Path(__file__).parents[1] / 'tools/flux_tessera_image.py'
    spec = importlib.util.spec_from_file_location('flux_tessera_image', path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def test_tessera_uses_landed_cpu_gate_resolution():
    images = Path(__file__).parents[1] / 'tools/images'
    pins = (images / 'tessera.txt').read_text()
    for pin in ('compressed-tensors==0.20.1', 'safetensors==0.6.2',
                'transformers==5.12.1', 'pip==25.1.1', 'wheel==0.48.0',
                'prismaquant @ https://github.com/RobTand/prismaquant/archive/'
                'f1335b0333749c1f400934b9e0f6ac06a99cf706.tar.gz'):
        assert pin + '\n' in pins
    assert 'prismabuild' not in pins.lower()
    assert 'tessera-quant' not in pins, 'the tested Tessera must come from PYTHONPATH=src'
    manifest, inputs = flux_builder().load_inputs(images)
    assert manifest['tessera_commit'] == '3f707db1fdf7ee6bc05f4d181c8b1cabbd162a59'
    assert manifest['requirements_sha256'] == hashlib.sha256(inputs['tessera.txt']).hexdigest()


def test_flux_builder_refuses_changed_requirements_before_podman(tmp_path):
    images = tmp_path / 'images'
    shutil.copytree(Path(__file__).parents[1] / 'tools/images', images)
    with (images / 'tessera-gate/gate_requirements.txt').open('ab') as stream:
        stream.write(b'pytest==9.0.2\n')
    with pytest.raises(ValueError, match='input hash mismatch: tessera-gate/gate_requirements.txt'):
        flux_builder().load_inputs(images)


def test_flux_builder_replaces_venv_and_checks_dependencies():
    script = flux_builder().install_script()
    assert script.index('rm -rf /opt/tl3') < script.index('python3 -m venv /opt/tl3')
    assert 'pip==25.1.1' in script
    assert '--report /receipts/bootstrap-report.json' in script
    assert '--require-hashes' in script and '--no-build-isolation' in script
    assert 'https://download.pytorch.org/whl/cpu' in script
    assert 'pip check' in script
    assert 'TESSERA_STRICT_CUDA' not in script
    assert 'apt-get' not in script


def test_flux_registry_verification_hashes_manifest_and_config(monkeypatch):
    builder = flux_builder()
    config = json.dumps({'architecture': 'amd64', 'os': 'linux'}).encode()
    config_digest = 'sha256:' + hashlib.sha256(config).hexdigest()
    manifest = json.dumps({'schemaVersion': 2, 'config': {'digest': config_digest}}).encode()
    digest = 'sha256:' + hashlib.sha256(manifest).hexdigest()
    blobs = {'manifests/' + digest: manifest, 'blobs/' + config_digest: config}
    monkeypatch.setattr(builder, 'registry_read', lambda suffix: blobs[suffix])
    assert builder.verify_registry(digest, config_digest) == {
        'manifest_digest': digest, 'config_digest': config_digest, 'platform': 'linux/amd64'}
    blobs['manifests/' + digest] = b'{}'
    with pytest.raises(ValueError, match='registry manifest digest mismatch'):
        builder.verify_registry(digest, config_digest)


def test_flux_registry_verification_rejects_wrong_config(monkeypatch):
    builder = flux_builder()
    config_digest = 'sha256:' + 'a' * 64
    manifest = json.dumps({'config': {'digest': config_digest}}).encode()
    digest = 'sha256:' + hashlib.sha256(manifest).hexdigest()
    monkeypatch.setattr(builder, 'registry_read', lambda suffix: manifest if suffix.startswith('manifests/') else b'{}')
    with pytest.raises(ValueError, match='registry config digest mismatch'):
        builder.verify_registry(digest, config_digest)


def pq_builder():
    spec = importlib.util.spec_from_file_location('flux_prismaquant_image', Path(__file__).parents[1] / 'tools/flux_prismaquant_image.py')
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def test_pq_image_uses_the_p8_hashed_cpu_lock():
    images = Path(__file__).parents[1] / 'tools/images'
    pins = (images / 'prismaquant.txt').read_text()
    assert 'torch==2.15.0+cpu' in pins
    assert 'pytest==9.1.1' in pins
    assert 'prismabuild' not in pins.lower()
    assert '--hash=sha256:' in pins


def test_pq_builder_requires_an_immutable_python312_base():
    builder = pq_builder()
    for base in ['python:3.12-slim-bookworm', 'python:3.14@sha256:' + '1' * 64]:
        with pytest.raises(ValueError, match='Python 3.12 base'):
            builder.base_reference(base)
    assert builder.base_reference('docker.io/library/python:3.12-slim-bookworm@sha256:' + '1' * 64)


def test_pq_install_keeps_the_subject_in_the_runtime_checkout():
    script = pq_builder().install_script()
    assert '--require-hashes --no-deps' in script
    assert '-r /input/requirements_linux_x86_64.txt' in script
    assert 'sys.version_info[:2] == (3, 12)' in script
    assert 'pip check' in script
    assert 'pip install -e' not in script
    assert 'prismabuild' in script
    assert 'affinity changed' in script


def test_pq_plan_consumer_keeps_node_deselections_and_refuses_empty_selection(tmp_path):
    builder = pq_builder()
    plan = {'schema': 'prismaquant.gate_plan.v1', 'base_sha': '1' * 40, 'head_sha': '2' * 40,
            'selected': [{'path': 'tests/test_flux_jobs.py'}],
            'deselect': ['tests/test_flux_jobs.py::test_deferred'],
            'deferred': [{'path': 'tests/test_cuda.py', 'lane': 'cuda'}]}
    argv = builder.pytest_argv(plan, base='1' * 40, head='2' * 40)
    assert argv[:5] == ['python', '-B', '-m', 'pytest', 'tests/test_flux_jobs.py']
    assert argv[argv.index('-n') + 1] == '2'
    assert '--timeout=600' in argv and '--dist=worksteal' in argv
    assert '--deselect=tests/test_flux_jobs.py::test_deferred' in argv
    assert '--junitxml=/output/junit.xml' in argv
    assert 'cache_dir=/scratch/pytest-cache' in argv
    assert plan['deferred'], 'caller must retain and report deferred lanes'
    plan['selected'] = []
    with pytest.raises(ValueError, match='empty selection'):
        builder.pytest_argv(plan, base='1' * 40, head='2' * 40)


def test_legacy_image_builder_refuses_pq_python314_before_container_commands(monkeypatch):
    commands = []
    monkeypatch.setenv('FLUX_JOB_ID', 'f123')
    monkeypatch.setattr(sys, 'argv', ['prepare_image.py', 'sha256:' + '1' * 64, '--profile', 'prismaquant'])
    monkeypatch.setattr(subprocess, 'run', lambda command, **kwargs: commands.append(command))
    with pytest.raises(ValueError, match='flux_prismaquant_image'):
        prepare_image.main()
    assert commands == []


def test_pq_builder_refuses_changed_hashed_lock(tmp_path):
    images = Path(__file__).parents[1] / 'tools/images'
    shutil.copytree(images, tmp_path / 'images')
    lock = tmp_path / 'images/prismaquant.txt'
    lock.write_bytes(lock.read_bytes() + b'\n# changed\n')
    with pytest.raises(ValueError, match='input hash'):
        pq_builder().load_inputs(tmp_path / 'images')


@pytest.mark.parametrize('damage', ['identity', 'traversal', 'foreign-node'])
def test_pq_plan_consumer_refuses_unbound_or_escaping_selection(damage):
    plan = {'schema': 'prismaquant.gate_plan.v1', 'base_sha': '1' * 40, 'head_sha': '2' * 40,
            'selected': [{'path': 'tests/test_flux_jobs.py'}], 'deselect': [], 'deferred': []}
    if damage == 'identity':
        plan['head_sha'] = '3' * 40
    elif damage == 'traversal':
        plan['selected'] = [{'path': 'tests/../../outside.py'}]
    else:
        plan['deselect'] = ['tests/test_other.py::test_other']
    with pytest.raises(ValueError, match='identity mismatch|invalid PQ test selector|selected module node'):
        pq_builder().pytest_argv(plan, base='1' * 40, head='2' * 40)
