"""Expose the locked Bazel Python environment to Rust's worker subprocesses."""
import os
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).parent))
from runtime import child_env, prepare  # noqa: E402


def main():
    prepare()
    os.environ['PYTHONPATH'] = os.pathsep.join(sys.path)
    os.execve(sys.executable, [sys.executable, *sys.argv[1:]], child_env())


if __name__ == '__main__':
    main()
