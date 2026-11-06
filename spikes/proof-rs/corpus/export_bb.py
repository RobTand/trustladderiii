"""Run only on BuildBuddy through bbrun step; install the standard harness edge."""
import os
from pathlib import Path
import subprocess
import sys

sys.path.insert(0, 'tools/ci')
from remote_gate import INSTALL_UV

subprocess.run(['bash', '-c', INSTALL_UV], check=True)
os.environ['PATH'] = str(Path.home() / '.local/bin') + ':' + os.environ['PATH']
os.environ['PYTHONPATH'] = ':'.join(str(Path(item).resolve()) for item in ('harness/src', 'spikes/proof-rs/corpus'))
os.environ['PYTEST_DISABLE_PLUGIN_AUTOLOAD'] = '1'
os.environ['PROOF_CORPUS_OUT'] = os.environ['BUILDBUDDY_ARTIFACTS_DIRECTORY']
result = subprocess.run(['uv', 'run', '--no-project', '--python', '3.14', '--with', 'pytest==9.0.2',
    '--with', 'bazel-runfiles', '--with', 'pytest-xdist==3.8.0', 'python', '-m', 'pytest', '-p', 'proof_corpus', '-q',
    *(sys.argv[1:] or ['harness/tests']), '--tb=short'])
raise SystemExit(result.returncode)
