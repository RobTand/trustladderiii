"""Source-only final checks on BuildBuddy, outside the production build graph."""
import hashlib
import io
import os
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
import urllib.request

sys.path.insert(0, 'tools/ci')
from remote_gate import INSTALL_UV

subprocess.run(['bash', '-c', INSTALL_UV], check=True)
os.environ['PATH'] = str(Path.home() / '.local/bin') + ':' + os.environ['PATH']
python = ['uv', 'run', '--no-project', '--python', '3.14', 'python']
steps = [
    ('python-lint', python + ['tools/ci/check.py', 'python-lint']),
    ('spike-python-lint', ['uv', 'tool', 'run', '--from', 'ruff==0.14.14', 'ruff', 'check', '--isolated', '--select', 'E9,F', 'spikes/proof-rs/corpus']),
    ('requirements', python + ['tools/bazel/requirements.py', '--check']),
    ('graph', python + ['tools/ci/graph_impact_check.py', '--base', '3eadde59269dd3fb39ea585d340ebfaa5eb81a0d', '--head', 'HEAD']),
    ('size', python + ['spikes/proof-rs/corpus/measure_size.py']),
]
failed = []
for name, command in steps:
    print('CHECK ' + name, flush=True)
    result = subprocess.run(command)
    if result.returncode:
        failed.append(name)
# A formatter component download does not compile or test the Rust package.
formatter_url = 'https://static.rust-lang.org/dist/rustfmt-1.99.1-x86_64-unknown-linux-gnu.tar.xz'
try:
    with urllib.request.urlopen(formatter_url, timeout=60) as response:
        data = response.read()
    with urllib.request.urlopen(formatter_url + '.sha256', timeout=60) as response:
        expected = response.read().decode().split()[0]
    if hashlib.sha256(data).hexdigest() != expected:
        raise ValueError('rustfmt archive checksum mismatch')
    with tempfile.TemporaryDirectory(prefix='spike902-fmt-') as temporary:
        archive = tarfile.open(fileobj=io.BytesIO(data), mode='r:xz')
        member = next(item for item in archive.getmembers() if item.name.endswith('/bin/rustfmt'))
        binary = Path(temporary) / 'rustfmt'
        binary.write_bytes(archive.extractfile(member).read())
        binary.chmod(0o755)
        result = subprocess.run([str(binary), '--check', '--edition', '2024', 'spikes/proof-rs/src/main.rs'])
        print('CHECK rustfmt-1.99.1', result.returncode, flush=True)
        if result.returncode:
            failed.append('rustfmt')
except (OSError, ValueError, StopIteration) as error:
    print('CHECK rustfmt unavailable:', str(error), flush=True)
    failed.append('rustfmt-unavailable')
print('FINAL_CHECK_FAILURES ' + repr(failed), flush=True)
raise SystemExit(bool(failed))
