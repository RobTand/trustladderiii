"""Trusted image profiles must install the dependency handoff, not the subject."""
import importlib.util
from pathlib import Path
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
    assert f'pytest=={pytest_version}\n' in pins
    assert f'triton=={triton_version}\n' in pins
    assert 'torch==2.11.0+cpu\n' in pins
    assert 'safetensors==0.8.0\n' in pins
    assert '--no-build-isolation' in script and '--no-deps' in script
    assert 'https://download.pytorch.org/whl/cpu' in script
    assert 'python3.14-dev' in script and 'g++' in script and 'make' in script
    assert script.index('setuptools==80.10.2') < script.index('--no-build-isolation')
    assert '-e .' not in script and 'pip install .' not in script
    assert commands[-1][:3] == ['docker', 'rm', '--force']
