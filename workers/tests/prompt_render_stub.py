"""Replay recorded Rust render replies. Only controller tests use --check."""
from functools import cache
import gzip
import json
from pathlib import Path
import subprocess
import sys


@cache
def cases():
    with gzip.open(Path(__file__).with_name('fixtures') / 'prompt-render.json.gz', 'rt') as source:
        return json.load(source)


def replay(kind, document):
    for row in cases():
        if row['kind'] == kind and row['document'] == document:
            return row['response']
    raise RuntimeError('unrecorded prompt renderer call')


def main():
    if len(sys.argv) == 3 and sys.argv[1] == '--check':
        for row in cases():
            result = subprocess.run([sys.argv[2], 'render-prompt', row['kind']],
                                    input=json.dumps(row['document']), text=True,
                                    capture_output=True, check=True)
            if json.loads(result.stdout) != row['response']:
                raise RuntimeError('Rust prompt recordings drifted')
        print(f'PASS {len(cases())} Rust prompt replay recordings checked')
    elif len(sys.argv) == 3 and sys.argv[1] == 'render-prompt':
        print(json.dumps(replay(sys.argv[2], json.load(sys.stdin))))
    else:
        raise RuntimeError('usage: prompt_render_stub.py --check BINARY or render-prompt KIND')


if __name__ == '__main__':
    main()
