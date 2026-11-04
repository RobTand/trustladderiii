"""Q32/#403 postmortem proof and mutant. Run only in an admitted disposable checkout.

`manifest` emits a PrismaBuild campaign until #726 supplies standalone Kani targets.
"""
import json
from pathlib import Path
import re
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[3]
SOURCE = ROOT / 'crates/controller/src/postmortem_reply/decision.rs'


def manifest():
    environment = {
        'PATH': '/home/rob/.cargo/bin:/usr/local/bin:/usr/bin:/bin',
        'RUSTUP_HOME': '/home/rob/.rustup', 'CARGO_HOME': '/home/rob/.cargo',
        'KANI_HOME': '/home/rob/.kani', 'CARGO_BUILD_JOBS': '1',
        'OMP_NUM_THREADS': '1', 'RAYON_NUM_THREADS': '1', 'RUST_TEST_THREADS': '1',
    }
    print(json.dumps([{
        'cwd': str(ROOT), 'argv': ['/usr/bin/python3', str(Path(__file__).relative_to(ROOT)), mode],
        'cpus': 1, 'demand': {'mem_gb': 2}, 'env': environment,
        'priority': -10, 'anywhere': True, 'timeout_s': 600,
    } for mode in ['prove', 'mutant']], indent=2))


def run(mutant):
    original = SOURCE.read_text()
    if mutant:
        before = 'Operation::Add => !item.before_nonempty && item.after_nonempty'
        assert original.count(before) == 1
        SOURCE.write_text(original.replace(before, 'Operation::Add => item.after_nonempty'))
        subprocess.run(['git', '-c', 'user.name=TL3 postmortem proof', '-c',
                        'user.email=noreply@anthropic.com', '-c', 'commit.gpgsign=false',
                        'commit', '-am', 'Seed postmortem add-operation mutant\n\n'
                        'Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>'],
                       cwd=ROOT, check=True)
        print('MUTANT SCRATCH COMMIT ' + subprocess.check_output(
            ['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(), flush=True)
    try:
        command = ['/usr/bin/time', '-v', 'kani',
                   'crates/controller/src/postmortem_reply/kani.rs', '--harness',
                   'proofs::postmortem_total_and_sound', '--exact', '--output-format', 'regular',
                   '-j', '1']
        process = subprocess.Popen(command, cwd=ROOT, stdout=subprocess.PIPE,
                                   stderr=subprocess.STDOUT, text=True)
        lines = []
        for line in process.stdout:
            print(line, end='', flush=True)
            lines.append(line)
        status = process.wait()
        output = ''.join(lines)
        if mutant:
            assert status != 0 and 'VERIFICATION:- FAILED' in output
            assert 'postmortem must satisfy count and operation rules' in output
            print('MUTANT REJECTED', flush=True)
        else:
            assert status == 0, status
            covers = re.findall(r'\*\* (\d+) of (\d+) cover properties satisfied', output)
            assert covers and all(hit == total for hit, total in covers)
            print('ALL COVERS REACHED', flush=True)
    finally:
        SOURCE.write_text(original)


if __name__ == '__main__':
    mode = sys.argv[1]
    if mode == 'manifest':
        manifest()
    elif mode in {'prove', 'mutant'}:
        run(mode == 'mutant')
    else:
        sys.exit('expected manifest, prove or mutant')
