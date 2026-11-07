"""Check requirements using remote_gate.py's pinned uv archive inside PB."""
import ast
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
import urllib.request

output = Path(sys.argv[1])
output.mkdir()
pins = {
    node.targets[0].id: ast.literal_eval(node.value)
    for node in ast.parse(Path('tools/ci/remote_gate.py').read_text()).body
    if isinstance(node, ast.Assign) and isinstance(node.targets[0], ast.Name)
    and node.targets[0].id in ('UV_VERSION', 'UV_SHA256')
}
with tempfile.TemporaryDirectory(prefix='fx-scratch-requirements-') as temporary:
    root = Path(temporary)
    archive = root / 'uv.tar.gz'
    urllib.request.urlretrieve(
        'https://github.com/astral-sh/uv/releases/download/' + pins['UV_VERSION']
        + '/uv-x86_64-unknown-linux-gnu.tar.gz', archive)
    actual = hashlib.sha256(archive.read_bytes()).hexdigest()
    assert actual == pins['UV_SHA256']
    with tarfile.open(archive) as packed:
        packed.extractall(root, filter='data')
    environment = {**os.environ, 'PATH': str(root / 'uv-x86_64-unknown-linux-gnu')
                   + ':' + os.environ.get('PATH', '')}
    with (output / 'uv-version.log').open('wb') as log:
        subprocess.run(['uv', '--version'], env=environment, stdout=log,
                       stderr=subprocess.STDOUT, check=True)
    with (output / 'requirements.log').open('wb') as log:
        result = subprocess.run([sys.executable, 'tools/bazel/requirements.py', '--check'],
                                env=environment, stdout=log, stderr=subprocess.STDOUT)
    (output / 'verdict.json').write_text(json.dumps({
        'requirements_exit': result.returncode, 'pins': pins, 'archive_sha256': actual,
        'source_head': subprocess.check_output(['git', 'rev-parse', 'HEAD']).decode().strip()}))
    print((output / 'requirements.log').read_text())
    print((output / 'verdict.json').read_text())
    sys.exit(result.returncode)
