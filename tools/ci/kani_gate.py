"""Prove the kernel's Kani harnesses on BuildBuddy and report each verdict.

Red stub for #726: the tests in test_kani_gate.py describe the behaviour.
"""
from pathlib import Path
import sys

from child_env import child_env  # noqa: F401

ROOT = Path(__file__).resolve().parents[2]
BUDGETS = Path(__file__).with_name('kani-budgets.json')
SCHEMA = 'tl3.kani-shard.v1'
RESULT = 'kani-shard.json'
CPUS = 4
MEMORY_GB = 44
RESERVED_GB = 2
MIN_TIMEOUT = 15 * 60
ISSUE_TITLE = 'Nightly Kani run failed on main'


class KaniError(RuntimeError):
    pass


def _todo(*_args, **_kwargs):
    raise NotImplementedError('#726: the Kani gate is not implemented')


harnesses = triggered = changed_files = load_budgets = budget = plan = verdict = _todo
shard_main = shard_entrypoint = run_request = cancel_recorded = check_shard = prove = _todo
summarize = gate_main = report = _todo


class Prover:
    def __init__(self, *args, **kwargs):
        _todo()


if __name__ == '__main__':
    sys.exit(_todo())
