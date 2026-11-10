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
    ('prismaquant', '9.1.1', '3.6.0'), ('tessera', '8.4.2', '3.8.0'),
])
def test_repository_profile_installs_frozen_cpu_dependencies(monkeypatch, profile, pytest_version, triton_version):
    commands = []
    monkeypatch.setenv('PRISMABUILD_CONTAINER_OWNER', 'fixture')
    monkeypatch.setenv('PRISMABUILD_CONTAINER_MARKER', 'fixture')
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
