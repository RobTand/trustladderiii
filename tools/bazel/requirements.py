"""Export or check pip.parse inputs against the authoritative uv lockfiles."""
import argparse
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[2]
PROJECTS = [
    ('workers', 'requirements.txt', ['--no-emit-package', 'tl3-harness']),
]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--check', action='store_true')
    args = parser.parse_args()
    stale = []
    with tempfile.TemporaryDirectory() as temporary:
        for project, name, extra in PROJECTS:
            output = Path(temporary, name)
            subprocess.run(['uv', 'export', '--project', str(ROOT / project), '--locked',
                            '--no-emit-project', '--no-header', '--no-annotate',
                            '--format', 'requirements-txt', *extra, '--output-file', str(output)],
                           check=True, stdout=subprocess.DEVNULL)
            committed = ROOT / 'tools/bazel' / name
            if args.check:
                if committed.read_bytes() != output.read_bytes():
                    stale.append(name)
            else:
                committed.write_bytes(output.read_bytes())
    if stale:
        raise SystemExit('uv exports drifted; run python3 tools/bazel/requirements.py: ' + ', '.join(stale))


if __name__ == '__main__':
    main()
