"""Measured delivery proofs and real-source mutant qualification, on remote compute."""
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import time


# Each seed changes executable source, with the contract left intact.
MUTANTS = {
    'fresh_sent': ('dispatch', 'start', 'State::Sent => Action::Transition(State::Uncertain),', 'State::Sent => intended(view),'),
    'pending_absent': ('converges', 'reconciled', 'Evidence::Absent => {', 'Evidence::Absent | Evidence::Pending => {'),
    'predecessor_bypass': ('approval', 'history', 'if predecessor { Stage::SentAmbiguous }', 'if false { Stage::SentAmbiguous }'),
    'retain_grant': ('approval', 'commit_sent', 'spend_grant: view.approval != ApprovalGate::Ordinary,', 'spend_grant: false,'),
    'changed_revision': ('dispatch', 'observed', 'if revision_changed { return Action::Return; }', 'if false { return Action::Return; }'),
    'omit_bridge': ('core', 'completion', 'let bridge = state == State::Uncertain && ack != Ack::Confirmed;', 'let bridge = false;'),
    'omit_retirement': ('converges', 'consume', 'let retire_retry = view.kind == DeliveryKind::KernelEffect && ack == Ack::Retryable && plan.to == State::Intended;', 'let retire_retry = false;'),
    'reject_equal': ('fence', 'accepts_fence', '!matches!(order, EpochOrder::Older)\n}', 'matches!(order, EpochOrder::Newer)\n}'),
    'admit_older': ('fence', 'accepts_fence', '!matches!(order, EpochOrder::Older)\n}', 'true\n}'),
}


def verify(verifier, z3, rustc, source, selection=None, extra=()):
    sysroot = Path(rustc).resolve().parents[1]
    command = [str(Path(verifier).resolve()), '--num-threads', '1', '--rlimit', '30',
               '--multiple-errors', '0', '-V', 'spinoff-all', '--sysroot', str(sysroot),
               str(source), '--edition', '2024', *extra]
    if selection:
        command += ['--verify-only-module', 'delivery', '--verify-function', selection]
    started = time.monotonic()
    try:
        result = subprocess.run(command, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                                timeout=40, check=False,
                                env={'PATH': str(sysroot / 'bin') + ':/usr/bin:/bin',
                                     'VERUS_Z3_PATH': str(Path(z3).resolve()),
                                     'LD_LIBRARY_PATH': str(sysroot / 'lib'), 'VERUS_USE_RUSTUP': '0',
                                     'OMP_NUM_THREADS': '1', 'RAYON_NUM_THREADS': '1'})
    except subprocess.TimeoutExpired as error:
        raise RuntimeError('proof timed out; this is not mutant rejection') from error
    return result, time.monotonic() - started


def main():
    verifier, z3, rustc, source, expected, label, mode = sys.argv[1:]
    source = Path(source)
    expected = int(expected)
    if mode == 'help':
        result, _ = verify(verifier, z3, rustc, source, extra=['--help'])
        print(result.stdout)
        return result.returncode
    result, seconds = verify(verifier, z3, rustc, source)
    print(result.stdout, flush=True)
    count = re.search(r'verification results:: (\d+) verified, 0 errors', result.stdout)
    if result.returncode or not count or int(count[1]) != expected:
        raise RuntimeError(f'baseline failed or obligation count differs from {expected}; wall={seconds:.3f}s')
    receipt = {'label': label, 'seconds': seconds, 'verified': int(count[1]), 'rlimit_budget': 30}
    if mode.startswith('mutant:'):
        name = mode.split(':', 1)[1]
        group, obligation, before, after = MUTANTS[name]
        with tempfile.TemporaryDirectory(prefix='delivery-mutant-') as scratch:
            root = Path(scratch) / 'delivery'
            shutil.copytree(source.parent.parent, root)
            path = root / 'src/step.rs'
            original = path.read_text()
            # Restrict to executable code, so a contract cannot be mutated too.
            start = original.index('fn ' + obligation + '(') if obligation != 'history' and obligation != 'observed' and obligation != 'consume' else original.index('fn ' + obligation + "<'a>(")
            end = original.find('\n#[cfg_attr', start)
            end = len(original) if end < 0 else end
            body = original[start:end]
            if body.count(before) != 1:
                raise RuntimeError(f'{name}: seed must match exactly once in {obligation}')
            body = body.replace(before, after)
            if name == 'pending_absent':
                body = body.replace('Evidence::Pending => {', 'Evidence::Pending if false => {')
            path.write_text(original[:start] + body + original[end:])
            result, elapsed = verify(verifier, z3, rustc, root / 'proofs' / (group + '.rs'), obligation)
            print(result.stdout, flush=True)
            if (result.returncode == 0 or not re.search(r'verification results:: \d+ verified, [1-9]\d* errors', result.stdout)
                    or not re.search(r'postcondition not satisfied|assertion failed|precondition not satisfied', result.stdout)
                    or re.search(r'error\[E\d+\]|aborting due to|unrecognized option', result.stdout)):
                raise RuntimeError(f'{name}: expected obligation failure, never a frontend error or timeout')
            receipt['mutant'] = {'name': name, 'obligation': 'delivery::' + obligation,
                                 'rejection': 'verification obligation', 'seconds': elapsed}
    print('DELIVERY_VERUS ' + json.dumps(receipt, sort_keys=True), flush=True)
    output = os.environ.get('TEST_UNDECLARED_OUTPUTS_DIR')
    if output:
        Path(output, 'delivery-verus.json').write_text(json.dumps(receipt, indent=2) + '\n')
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
