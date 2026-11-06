"""Measured delivery proofs and real-source mutant qualification, on remote compute."""
import hashlib
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
    'pending_absent': ('converges', 'reconciled', 'match evidence {', 'match if evidence == Evidence::Pending { Evidence::Absent } else { evidence } {'),
    'predecessor_bypass': ('approval', 'history', 'if predecessor {\n                Stage::SentAmbiguous', 'if false {\n                Stage::SentAmbiguous'),
    'retain_grant': ('approval', 'commit_sent', 'spend_grant: view.approval != ApprovalGate::Ordinary,', 'spend_grant: false,'),
    'changed_revision': ('dispatch', 'observed', 'if revision_changed {', 'if false {'),
    'omit_bridge': ('core', 'completion', 'let bridge = state == State::Uncertain && ack != Ack::Confirmed;', 'let bridge = false;'),
    'omit_retirement': ('converges', 'consume', 'let retire_retry = view.kind == DeliveryKind::KernelEffect\n        && ack == Ack::Retryable\n        && plan.to == State::Intended;', 'let retire_retry = false;'),
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
    result, seconds = verify(verifier, z3, rustc, source, extra=['--time', '--output-json'])
    stats = None
    for candidate in re.finditer(r'(?m)^\{\s*$', result.stdout):
        try:
            value, _ = json.JSONDecoder().raw_decode(result.stdout[candidate.start():])
        except json.JSONDecodeError:
            continue
        if isinstance(value, dict) and 'verification-results' in value:
            stats = value
            break
    if stats is None:
        print(result.stdout, flush=True)
        raise RuntimeError('verifier did not emit a result object')
    verdict = stats['verification-results']
    if result.returncode or not verdict['success'] or verdict['verified'] != expected or verdict['errors']:
        print(result.stdout, flush=True)
        raise RuntimeError(f'baseline failed or obligation count differs from {expected}; wall={seconds:.3f}s')
    timing = stats['times-ms']
    smt = timing['smt']
    functions = [f for m in smt['smt-run-module-times'] for f in m['function-breakdown']]
    receipt = {'label': label, 'seconds': seconds, 'verified': verdict['verified'],
               'verus_seconds': timing['total'] / 1000, 'rlimit_budget': 30,
               'rlimit_init': smt['rlimit-init'], 'rlimit_run': smt['rlimit-run'],
               'max_function_rlimit': max(f['rlimit'] for f in functions)}
    if seconds > 20:
        raise RuntimeError(f'{label}: group exceeds the hard 20s ceiling ({seconds:.3f}s)')
    if mode.startswith('mutant:'):
        name = mode.split(':', 1)[1]
        group, obligation, before, after = MUTANTS[name]
        with tempfile.TemporaryDirectory(prefix='delivery-mutant-') as scratch:
            root = Path(scratch) / 'delivery'
            shutil.copytree(source.parent.parent, root)
            path = root / 'src/step.rs'
            original = path.read_text()
            # Restrict to executable code, so a contract cannot be mutated too.
            match = re.search(r'\bfn ' + obligation + r'(?:<[^>]*>)?\s*\(', original)
            if match is None:
                raise RuntimeError(f'{name}: executable obligation disappeared')
            start = match.start()
            end = original.find('\n#[cfg_attr', start)
            end = len(original) if end < 0 else end
            body = original[start:end]
            if body.count(before) != 1:
                raise RuntimeError(f'{name}: seed must match exactly once in {obligation}')
            body = body.replace(before, after)
            path.write_text(original[:start] + body + original[end:])
            result, elapsed = verify(verifier, z3, rustc, root / 'proofs' / (group + '.rs'), obligation)
            print(result.stdout, flush=True)
            if (result.returncode == 0 or not re.search(r'verification results:: \d+ verified, [1-9]\d* errors', result.stdout)
                    or not re.search(r'postcondition not satisfied|assertion failed|precondition not satisfied', result.stdout)
                    or re.search(r'error\[E\d+\]|unrecognized option', result.stdout)):
                raise RuntimeError(f'{name}: expected obligation failure, never a frontend error or timeout')
            receipt['mutant'] = {'name': name, 'obligation': 'delivery::' + obligation,
                                 'source_sha256': hashlib.sha256(path.read_bytes()).hexdigest(),
                                 'rejection': 'verification obligation', 'seconds': elapsed}
    inventory = {'proofs': {label: {'kind': 'verus', 'expected_obligations': expected,
                                    'verified': verdict['verified'], 'threads': 1,
                                    'seconds': seconds, 'module': mode}}}
    print('VERUS_PROOF ' + json.dumps(inventory, sort_keys=True), flush=True)
    print('DELIVERY_VERUS ' + json.dumps(receipt, sort_keys=True), flush=True)
    output = os.environ.get('TEST_UNDECLARED_OUTPUTS_DIR')
    if output:
        Path(output, 'inventory.json').write_text(json.dumps(inventory) + '\n')
        Path(output, 'delivery-verus.json').write_text(json.dumps(receipt, indent=2) + '\n')
        Path(output, 'verifier.json').write_text(json.dumps(stats, indent=2) + '\n')
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
