"""Source-only final checks on BuildBuddy, outside the production build graph."""
import os
from pathlib import Path
import subprocess
import sys

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
print('FINAL_CHECK_FAILURES ' + repr(failed), flush=True)
raise SystemExit(bool(failed))
