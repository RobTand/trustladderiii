"""Admitted fixture: real Rust input and staged verifier, synthetic host/generation."""
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'src'))
from tl3_harness import runtime

input_path, output = Path(sys.argv[1]), Path(sys.argv[2])
output.mkdir(parents=True)
raw = input_path.read_bytes()
facts = json.loads(raw)
with tempfile.TemporaryDirectory(prefix='scratch-front-door-') as temporary:
    tree = Path(temporary)
    systemctl = tree / 'systemctl-fixture'
    systemctl.write_text(f'#!{sys.executable}\nprint({facts["run"]["instance"].split("/")[-1]!r})\n')
    systemctl.chmod(0o700)
    _, inventory, package = runtime.stage_host_runtime(
        tree / 'staged', interpreter=sys.executable, producer_commit='d' * 40,
        podman='/__must_not_execute_podman__', systemctl=str(systemctl))
    wrapper = inventory.parent / 'host-cleanup'
    shim = '''import os, runpy, sys
from types import SimpleNamespace
os.uname = lambda: SimpleNamespace(nodename=sys.argv[2])
sys.argv = [sys.argv[1]]
runpy.run_path(sys.argv[0], run_name='__main__')
'''
    result = subprocess.run([sys.executable, '-I', '-B', '-c', shim, str(wrapper), facts['host']],
                            input=raw, capture_output=True, check=False)
    for name in ('request.json', 'intent.json'):
        shutil.copyfile(input_path.parent / name, output / name)
    (output / 'rust-input.json').write_bytes(raw)
    (output / 'stdout.json').write_bytes(result.stdout)
    (output / 'stderr.log').write_bytes(result.stderr)
    (output / 'inventory.json').write_bytes(inventory.read_bytes())
    (output / 'host-cleanup').write_bytes(wrapper.read_bytes())
    (output / 'runtime.py').write_bytes((package / 'runtime.py').read_bytes())
    assert result.returncode == 0, result.stderr
    observed = json.loads(result.stdout)
    assert observed['observation'] == {'kind': 'absent', 'errno': 2}
    assert observed['run'] == facts['run'] and observed['nonce'] == facts['nonce']
    assert observed['attribution'] is None
    (output / 'assertions.json').write_text(json.dumps({
        'actual_rust_input': True, 'real_staged_verifier_and_front_door': True,
        'fixture_host_and_owned_generation_executable': True, 'returncode': result.returncode}))
