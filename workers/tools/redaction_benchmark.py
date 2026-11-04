"""Measure fresh Rust CLI calls against Python matching on planted span text.

Run inside one PrismaBuild measurement action after building `tl3`:
PYTHONPATH=workers/src python3.14 workers/tools/redaction_benchmark.py target/debug/tl3
"""
import json
from pathlib import Path
import statistics
import subprocess
import sys
import time

from tl3_workers.tracing import Redactor


def main():
    binary = str(Path(sys.argv[1]).resolve())
    redactor = Redactor(None)
    texts = [
        'tool finished successfully; modified src/main.rs; 12 lines changed',
        'password: zE9!pR4#cV7$\nordinary log line\n',
        json.dumps({'command': 'curl -H "Authorization: Bearer sk-fixture7530123456789abcdef" https://example.invalid'}),
    ]
    samples = {'python': [], 'rust_cli': []}
    # Alternate order within one admitted measurement so drift affects both arms.
    for turn in range(24):
        text = texts[turn % len(texts)]
        expected = redactor.text(text, limit=len(text))
        for arm in (('python', 'rust_cli') if turn % 2 else ('rust_cli', 'python')):
            start = time.perf_counter_ns()
            if arm == 'python':
                actual = redactor.text(text, limit=len(text))
            else:
                actual = subprocess.run([binary, 'redact'], input=text, text=True, capture_output=True,
                                        check=True, env={}).stdout
            samples[arm].append((time.perf_counter_ns() - start) / 1_000_000)
            if actual != expected:
                raise RuntimeError('redaction output mismatch on planted benchmark input')
    print(json.dumps({'iterations_per_engine': 24, 'units': 'milliseconds_per_span',
                      'python': sys.version.split()[0],
                      'span_bytes': [len(text.encode()) for text in texts],
                      'results': {arm: {'median': statistics.median(values),
                                        'min': min(values), 'max': max(values)}
                                  for arm, values in samples.items()}}, indent=2))


if __name__ == '__main__':
    main()
