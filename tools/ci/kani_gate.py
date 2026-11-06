"""Prove the kernel's Kani harnesses on BuildBuddy and report each verdict.

    kani_gate.py pr --base SHA --head SHA --branch NAME
        Gate job. Proves the measured PR tier when the change can affect a proof,
        and proves nothing otherwise.
    kani_gate.py nightly --head SHA --branch main
        Proves every harness and reports per-harness verdicts and times.
    kani_gate.py budgets --measurements FILE --output FILE
        Derives tiers from measured per-harness records.
    kani_gate.py cancel
        Cancels the shard runs this job recorded.
    kani_gate.py report --summary FILE
        Opens, or comments on, the nightly regression issue.
    kani_gate.py shard --head SHA --mode MODE --index N HARNESS...
        Runner side. Installs the pinned Kani, then proves the named harnesses.

Each shard is one BuildBuddy Remote Bazel runner, started through the Run API.
It proves its harnesses in parallel, within the runner's CPUs and memory, and
leaves kani-shard.json as a BuildBuddy artifact. A harness passes only when
Kani exits 0, reports VERIFICATION:- SUCCESSFUL and satisfies every cover
check. A failure, an unsatisfied cover, a missing result or a harness that
reaches its hard timeout fails the run. Running past the measured warning
budget is a warning only.

Budgets come from tools/ci/kani-budgets.json, measured on BuildBuddy. The PR
gate stops at the first failing harness and cancels the other shards.
"""
import argparse
from fnmatch import fnmatch
import json
import math
import os
from pathlib import Path
import re
import shlex
import shutil
import signal
import subprocess
import sys
import threading
import time
import urllib.request

from child_env import child_env

ROOT = Path(__file__).resolve().parents[2]
BUDGETS = Path(__file__).with_name('kani-budgets.json')
SCHEMA = 'tl3.kani-shard.v1'
RESULT = 'kani-shard.json'
KANI_VERSION = '0.68.0'
RUST_VERSION = '1.99.1'
# A change matching any of these can change a proof's verdict or how the gate
# proves it. fnmatch's * also crosses '/'. Dependencies are added from Cargo
# metadata, including optional, build, dev and target-specific dependencies.
TRIGGERS = [
    'crates/kernel/src/*', 'crates/kernel/Cargo.toml', 'crates/kernel/build.rs',
    'Cargo.toml', 'Cargo.lock', 'rust-toolchain', 'rust-toolchain.toml',
    'tools/ci/kani_gate.py', 'tools/ci/test_kani_gate.py', 'tools/ci/kani-budgets.json',
    'tools/ci/remote_gate.py', 'tools/ci/child_env.py',
    '.github/workflows/gate.yml', '.github/workflows/kani-nightly.yml',
]
# Runner size. CBMC is single-threaded, so each running harness takes one CPU.
CPUS = 8
MEMORY_GB = 176
# Leave room for the runner's own processes inside the VM.
RESERVED_GB = 2
# BuildBuddy's free tier kills every Remote Bazel run at one hour, whatever
# timeout the Run API asks for (invocation 3d5ffcdf-9f3b-431e-9f0c-ac31e13e262a:
# "Remote run exceeded timeout (1h0m0s) due to free tier limitations"). A
# killed runner uploads nothing, so every shard must finish inside the hour.
RUNNER_MINUTES = 60
# Install and codegen: about 4 minutes on a warm runner, more on a cold one.
SETUP_SECONDS = 8 * 60
# What a shard may spend proving, with two minutes for the upload and scan.
PROVE_SECONDS = RUNNER_MINUTES * 60 - SETUP_SECONDS - 120
# Planned finish of every shard, leaving room for a slower executor.
TARGET_SECONDS = int(PROVE_SECONDS * 0.9)
MAX_SHARDS = 12
PR_SECONDS = 600
PR_TARGET_SECONDS = 18 * 60
# Shards beyond the account's concurrent quota queue, so the job waits for
# about three runner hours in total.
WAIT_MINUTES = 180
PR_WAIT_MINUTES = 50
# Assumed for a harness with no measurement yet. It runs alone on its runner.
UNMEASURED_SECONDS = 1800.0
# Measured peaks repeat closely from run to run; 20% headroom lets four of the
# 35 GB proofs share a runner.
MEMORY_HEADROOM = 1.2
# The hard timeout fails a harness, as Bazel's TIMEOUT does. It is generous,
# because a shared executor can be slow, but no later than the runner allows.
# The warning budget is the measure.
TIMEOUT_FACTOR = 3
MIN_TIMEOUT = 15 * 60
WARN_FACTOR = 1.5
HEARTBEAT = 60
INSTALL_KANI = f'''set -euo pipefail
export PATH="$HOME/.cargo/bin:$PATH"
if ! command -v rustup >/dev/null; then
  setup=$(mktemp -d)
  curl --proto '=https' --tlsv1.2 -fsS -o "$setup/rustup-init" \\
    https://static.rust-lang.org/rustup/dist/x86_64-unknown-linux-gnu/rustup-init
  chmod +x "$setup/rustup-init"
  "$setup/rustup-init" -y --no-modify-path --profile minimal --default-toolchain {RUST_VERSION}
fi
rustup toolchain install {RUST_VERSION} --profile minimal
rustup default {RUST_VERSION}
if [ "$(cargo-kani --version 2>/dev/null | head -n 1)" != "Kani Rust Verifier {KANI_VERSION} (cargo plugin)" ]; then
  cargo install --locked kani-verifier --version {KANI_VERSION}
fi
cargo kani setup
cargo kani --version'''


class KaniError(RuntimeError):
    """The gate could not establish a verdict for every harness. It fails."""


def harnesses(source=None):
    """Read kernel Rust files, including moved modules and literal #[path] files.

    Scan conservatively rather than interpreting Rust cfg/module syntax. The
    budget inventory must match, so an unrecognized layout fails closed.
    """
    if source is None:
        pending = sorted((ROOT / 'crates/kernel').rglob('*.rs'))
        seen, sources = set(), []
        while pending:
            path = pending.pop(0).resolve()
            if path in seen:
                continue
            if not path.is_relative_to(ROOT.resolve()):
                raise KaniError(f'proof module outside the checkout: {path}')
            seen.add(path)
            text = path.read_text()
            sources.append(text)
            for relative in re.findall(r'#\s*\[\s*path\s*=\s*"([^"]+)"\s*\]', text):
                target = path.parent / relative
                if target.is_file():
                    pending.append(target)
        source = '\n'.join(sources)
    names = re.findall(r'^\s*proof!\(\s*(\w+),', source, re.M)
    names += re.findall(r'#\[kani::proof\]\s*(?:#\[[^\]]*\]\s*)*(?:pub\s+)?fn\s+(\w+)', source)
    if len(names) != len(set(names)):
        raise KaniError('could not list the kernel proof harnesses')
    return names


def kernel_dependencies():
    """Derive local dependency directories without fetching or compiling crates.

    --no-deps describes workspace packages, including their declared paths. If
    Cargo or a dependency description is unavailable, selection runs all proofs.
    """
    try:
        result = subprocess.run(['cargo', 'metadata', '--no-deps', '--locked', '--offline',
                                 '--format-version=1'], cwd=ROOT, check=True, capture_output=True,
                                text=True, timeout=30, env=child_env())
        packages = {Path(p['manifest_path']).parent.resolve(): p
                    for p in json.loads(result.stdout)['packages']}
        pending, seen = [(ROOT / 'crates/kernel').resolve()], set()
        while pending:
            path = pending.pop()
            if path in seen:
                continue
            seen.add(path)
            package = packages[path]
            pending.extend(Path(dep['path']).resolve() for dep in package['dependencies'] if dep.get('path'))
        return [path.relative_to(ROOT.resolve()).as_posix() + '/' for path in seen]
    except (OSError, ValueError, KeyError, subprocess.SubprocessError):
        return None


def triggered(changed):
    direct = {path for path in changed if any(fnmatch(path, pattern) for pattern in TRIGGERS)}
    if direct == set(changed):
        return sorted(direct)
    dependencies = kernel_dependencies()
    if dependencies is None:
        print('Kani: dependency metadata unavailable; conservatively proving the PR tier', flush=True)
        return sorted(direct | {path for path in changed if path.startswith('crates/')
                                or path.endswith(('.rs', '/Cargo.toml'))})
    dependencies = [path for path in dependencies if path != 'crates/kernel/']
    return sorted(direct | {path for path in changed if any(path.startswith(dep) for dep in dependencies)})


def changed_files(base, head):
    for sha in (base, head):
        if not sha:
            raise KaniError('empty base or head: the queue-aware refs were unavailable')
        present = subprocess.run(['git', 'cat-file', '-e', f'{sha}^{{commit}}'], cwd=ROOT,
                                 stderr=subprocess.DEVNULL, env=child_env())
        if present.returncode:
            subprocess.run(['git', 'fetch', '--quiet', '--no-tags', 'origin', sha], cwd=ROOT,
                           check=True, env=child_env())
    output = subprocess.run(['git', 'diff', '--name-only', '--no-renames', '-z', base, head],
                            cwd=ROOT, check=True, capture_output=True, text=True, env=child_env()).stdout
    return {path for path in output.split('\0') if path}


def build_budgets(measurements):
    """Keep measurements and derive tier membership on every remeasurement."""
    entries = {}
    for name, measured in measurements.items():
        entry = {key: value for key, value in measured.items() if key != 'tier'}
        entry['tier'] = 'pr' if entry['seconds'] <= PR_SECONDS and not entry.get('lower_bound') else 'nightly'
        entries[name] = entry
    return {'schema': 'tl3.kani-budgets.v1',
            'source': 'Measured on BuildBuddy, Kani 0.68.0, 8-CPU 176 GB runners unless noted by invocation; '
                      'seconds are wall time under the shard\'s own parallel load. '
                      'Tiers are generated by kani_gate.py budgets: pr <= 600s, nightly otherwise.',
            'harnesses': entries}


def acceptable(result):
    return result['verdict'] == 'SUCCESSFUL'


def load_budgets(path=BUDGETS):
    data = json.loads(path.read_text())
    if data.get('schema') != 'tl3.kani-budgets.v1':
        raise KaniError(f'{path.name} has the wrong schema')
    return data['harnesses']


def budget(name, budgets):
    """Planning estimate, memory reservation, warning budget and hard timeout."""
    measured = budgets.get(name)
    usable = MEMORY_GB - RESERVED_GB
    seconds = measured['seconds'] if measured else UNMEASURED_SECONDS
    peak = measured['peak_gb'] if measured else usable
    # A proof whose headroom would not fit runs alone on its runner.
    memory = min(usable, max(2, math.ceil(peak * MEMORY_HEADROOM)))
    timeout = min(PROVE_SECONDS, max(MIN_TIMEOUT, math.ceil(TIMEOUT_FACTOR * seconds)))
    return {'seconds': seconds, 'memory_gb': memory, 'peak_gb': peak, 'measured': bool(measured),
            'warn_after': round(WARN_FACTOR * seconds, 1), 'timeout': timeout}


def makespan(names, budgets, cpus=CPUS, memory_gb=MEMORY_GB):
    """Simulate Prover's schedule: longest first, first fit by memory and CPU."""
    free, clock, running = memory_gb - RESERVED_GB, 0.0, []
    pending = sorted(names, key=lambda name: (-budget(name, budgets)['seconds'], name))
    while pending or running:
        for name in list(pending):
            info = budget(name, budgets)
            if len(running) < cpus and info['memory_gb'] <= free:
                pending.remove(name)
                free -= info['memory_gb']
                running.append((clock + info['seconds'], info['memory_gb']))
        running.sort()
        clock, memory = running.pop(0)
        free += memory
    return clock


def plan(names, budgets, cpus=CPUS, memory_gb=MEMORY_GB, max_shards=MAX_SHARDS,
         target=TARGET_SECONDS):
    """Split harnesses across the fewest shards that each finish by target.

    Harnesses go longest first onto the least loaded shard. A harness that
    needs more than one CPU's share of memory counts that many slots. A
    harness that cannot finish inside a runner's hour fails the plan."""
    usable = memory_gb - RESERVED_GB
    share = usable / cpus
    load = {}
    for name in names:
        info = budget(name, budgets)
        if info['peak_gb'] > usable:
            raise KaniError(f'{name} peaks at {info["peak_gb"]} GB, more than a runner has')
        if info['seconds'] > target:
            raise KaniError(
                f'{name} takes {info["seconds"]:.0f}s, more than a {RUNNER_MINUTES}-minute runner can '
                f'prove ({target}s after setup). Split the harness, or raise the BuildBuddy plan\'s limit.')
        load[name] = info['seconds'] * max(1, math.ceil(info['memory_gb'] / share))
    if not names:
        return []
    count = max(1, min(max_shards, len(names), math.ceil(sum(load.values()) / cpus / target)))
    while True:
        shards = [[] for _ in range(count)]
        totals = [0.0] * count
        for name in sorted(names, key=lambda name: (-load[name], name)):
            index = min(range(count), key=lambda i: (totals[i], i))
            shards[index].append(name)
            totals[index] += load[name]
        shards = [shard for shard in shards if shard]
        longest = max(makespan(shard, budgets, cpus, memory_gb) for shard in shards)
        if longest <= target or count >= min(max_shards, len(names)):
            break
        count += 1
    if longest > target:
        raise KaniError(f'{len(names)} harnesses need more than {max_shards} shards to finish inside '
                        f'{RUNNER_MINUTES}-minute runners')
    return shards


# Kani's terse output: one block per harness.
VERDICT = re.compile(r'^VERIFICATION:- (SUCCESSFUL|FAILED)\s*$', re.M)
COVERS = re.compile(r'\*\* (\d+) of (\d+) cover properties satisfied')
VERIFY_TIME = re.compile(r'^Verification Time: ([0-9.]+)s', re.M)


def verdict(output, returncode, timed_out):
    """SUCCESSFUL only for a clean, complete, non-vacuous proof."""
    found = VERDICT.findall(output)
    covers = [(int(hit), int(total)) for hit, total in COVERS.findall(output)]
    times = VERIFY_TIME.findall(output)
    result = {'line': f'VERIFICATION:- {found[-1]}' if found else None,
              'covers': [sum(hit for hit, _ in covers), sum(total for _, total in covers)] if covers else None,
              'verification_seconds': float(times[-1]) if times else None}
    if timed_out and 'FAILED' not in found:
        return {**result, 'verdict': 'TIMEOUT', 'reason': 'reached its hard timeout'}
    if not found and (returncode in (137, -9, 143, -15) or re.search(r'out of memory|signal 9|killed', output, re.I)):
        return {**result, 'verdict': 'KILLED', 'reason': 'no VERIFICATION line; the solver was killed, likely out of memory'}
    if found != ['SUCCESSFUL']:
        reason = 'no VERIFICATION line' if not found else 'VERIFICATION:- FAILED'
        if not covers and (returncode in (137, -9) or re.search(r'out of memory|signal 9|killed', output, re.I)):
            # A killed solver is not a counterexample, but it still verified nothing.
            reason += '; the solver was killed, likely out of memory'
        return {**result, 'verdict': 'FAILED', 'reason': reason}
    if returncode:
        return {**result, 'verdict': 'FAILED', 'reason': f'kani exited {returncode}'}
    if not covers:
        return {**result, 'verdict': 'FAILED', 'reason': 'no cover results reported'}
    if any(hit < total for hit, total in covers):
        return {**result, 'verdict': 'FAILED',
                'reason': 'a cover check was not satisfied, so the proof may be vacuous'}
    return {**result, 'verdict': 'SUCCESSFUL', 'reason': None}


class Prover:
    """Runner side: prove harnesses in parallel within the runner's resources."""

    def __init__(self, names, budgets, logs, cpus=CPUS, memory_gb=MEMORY_GB, fail_fast=True,
                 command=None, clock=time.monotonic, poll=0.5):
        self.pending = sorted(names, key=lambda name: (-budget(name, budgets)['seconds'], name))
        self.budgets, self.logs = budgets, logs
        self.cpus, self.free = cpus, memory_gb - RESERVED_GB
        self.fail_fast, self.clock, self.poll = fail_fast, clock, poll
        self.command = command or (lambda name: [
            'cargo', 'kani', '-p', 'tl3-kernel', '--output-format', 'terse',
            '--harness', f'proofs::{name}', '--exact'])
        self.running, self.results, self.stopping = {}, {}, set()
        self.heartbeat = HEARTBEAT

    def start(self, name):
        info = budget(name, self.budgets)
        log = (self.logs / f'{name}.log').open('w')
        process = subprocess.Popen(self.command(name), cwd=ROOT, stdout=log, stderr=subprocess.STDOUT,
                                   env=child_env(), start_new_session=True)
        self.free -= info['memory_gb']
        self.running[process.pid] = (name, process, log, self.clock(), info)
        print(f'KANI start {name} timeout {info["timeout"]}s reserve {info["memory_gb"]} GB', flush=True)

    def finish(self, pid, status, usage, timed_out=False):
        name, process, log, started, info = self.running.pop(pid)
        process.returncode = os.waitstatus_to_exitcode(status)
        log.close()
        self.free += info['memory_gb']
        seconds = self.clock() - started
        output = (self.logs / f'{name}.log').read_text(errors='replace')
        result = verdict(output, process.returncode, timed_out)
        if pid in self.stopping:
            # Killed by fail-fast: no verdict, and not a solver failure either.
            result.update(verdict='STOPPED', reason='stopped after an earlier harness failed')
        result.update(name=name, seconds=round(seconds, 1), timeout=info['timeout'],
                      warn_after=info['warn_after'], measured=info['measured'],
                      cpu_seconds=round(usage.ru_utime + usage.ru_stime, 1),
                      # Linux reports kilobytes; this includes waited-for descendants.
                      peak_gb=round(usage.ru_maxrss / 2**20, 2),
                      warning=seconds > info['warn_after'])
        self.results[name] = result
        covers = '{}/{}'.format(*result['covers']) if result['covers'] else 'none'
        print(f'KANI {result["verdict"]} {name} {seconds:.1f}s covers {covers} '
              f'peak {result["peak_gb"]} GB{" WARNING over budget" if result["warning"] else ""}'
              f'{" (" + result["reason"] + ")" if result["reason"] else ""}', flush=True)
        # One machine-readable line per harness: the log keeps the result even
        # when the runner's artifact upload is lost.
        print('KANI_RESULT ' + json.dumps(result, sort_keys=True), flush=True)
        if result['verdict'] != 'SUCCESSFUL':
            print(f'KANI log tail {name}:\n' + output[-3000:], flush=True)
        if result['warning']:
            print(f'::warning::{name} took {seconds:.1f}s, over its {info["warn_after"]}s budget', flush=True)
        return result

    def kill(self, pid):
        try:
            os.killpg(pid, signal.SIGKILL)
        except ProcessLookupError:
            pass

    def run(self):
        stopped = False
        beat = self.clock()
        while self.pending or self.running:
            while not stopped and self.pending and len(self.running) < self.cpus:
                fits = [name for name in self.pending
                        if budget(name, self.budgets)['memory_gb'] <= self.free]
                if not fits:
                    break
                self.pending.remove(fits[0])
                self.start(fits[0])
            if not self.running:
                if self.pending and not stopped:
                    raise KaniError('a harness does not fit in the runner')
                break
            reaped = False
            for pid in list(self.running):
                done, status, usage = os.wait4(pid, os.WNOHANG)
                timed_out = False
                if not done:
                    name, _, _, started, info = self.running[pid]
                    if self.clock() - started < info['timeout']:
                        continue
                    self.kill(pid)
                    done, status, usage = os.wait4(pid, 0)
                    timed_out = True
                reaped = True
                result = self.finish(pid, status, usage, timed_out)
                if result['verdict'] != 'SUCCESSFUL' and self.fail_fast and not stopped:
                    stopped = True
                    for other in list(self.running):
                        self.stopping.add(other)
                        self.kill(other)
            if self.clock() - beat >= self.heartbeat:
                # A quiet runner log can lose its event stream; keep it moving.
                beat = self.clock()
                print('KANI running ' + ', '.join(
                    f'{name} {beat - started:.0f}s' for name, _, _, started, _ in self.running.values()),
                    flush=True)
            if not reaped:
                time.sleep(self.poll)
        for name in self.pending:
            self.results[name] = {'name': name, 'verdict': 'NOT_RUN', 'reason': 'an earlier harness failed'}
        self.pending = []
        return self.results


def logged(label, argv, path):
    """Run one setup step into its log, with a heartbeat while it is quiet."""
    started = time.monotonic()
    with path.open('w') as log:
        process = subprocess.Popen(argv, cwd=ROOT, stdout=log, stderr=subprocess.STDOUT, env=child_env())
        while True:
            try:
                return process.wait(timeout=HEARTBEAT)
            except subprocess.TimeoutExpired:
                print(f'KANI {label} running {time.monotonic() - started:.0f}s', flush=True)


def shard_main(args):
    """Runner child, with a scrubbed environment."""
    output = Path(os.environ.get('BUILDBUDDY_ARTIFACTS_DIRECTORY', ROOT / 'kani-artifacts'))
    logs = output / 'kani-logs'
    logs.mkdir(parents=True, exist_ok=True)
    head = subprocess.run(['git', 'rev-parse', 'HEAD'], cwd=ROOT, capture_output=True, text=True,
                          env=child_env()).stdout.strip()
    record = {'schema': SCHEMA, 'head': args.head, 'checkout': head, 'mode': args.mode,
              'index': args.index, 'harnesses': args.harness, 'results': {}, 'error': None}
    started = time.monotonic()
    try:
        if head != args.head:
            raise KaniError(f'runner checked out {head}, not {args.head}')
        unknown = set(args.harness) - set(harnesses())
        if unknown:
            raise KaniError('unknown harness: ' + ', '.join(sorted(unknown)))
        os.environ['PATH'] = f'{Path.home()}/.cargo/bin:' + os.environ.get('PATH', '')
        code = logged('setup', ['bash', '-c', INSTALL_KANI], logs / 'setup.log')
        print((logs / 'setup.log').read_text(errors='replace')[-4000:], flush=True)
        record['setup_seconds'] = round(time.monotonic() - started, 1)
        if code:
            raise KaniError(f'installing Kani {KANI_VERSION} exited {code}')
        # Build once, so parallel harnesses reuse the codegen instead of
        # waiting on Cargo's lock one after another.
        code = logged('codegen', ['cargo', 'kani', '-p', 'tl3-kernel', '--only-codegen'], logs / 'codegen.log')
        record['codegen_seconds'] = round(time.monotonic() - started - record['setup_seconds'], 1)
        if code:
            print((logs / 'codegen.log').read_text(errors='replace')[-4000:], flush=True)
            raise KaniError(f'Kani codegen exited {code}')
        budgets = load_budgets()
        prover = Prover(args.harness, budgets, logs, fail_fast=args.mode == 'pr')
        record['results'] = prover.run()
    except (KaniError, OSError, ValueError, KeyError) as error:
        record['error'] = str(error)
    record['seconds'] = round(time.monotonic() - started, 1)
    (output / RESULT).write_text(json.dumps(record, indent=2, sort_keys=True) + '\n')
    failed = record['error'] or any(not acceptable(r) for r in record['results'].values())
    return 1 if failed else 0


def scan(directory):
    """Only the scanner child receives the runner credential."""
    if os.environ.get('BBRUN_FINAL_SCAN') == '1':
        print('secret scan deferred to bbrun final scan', flush=True)
        return 0
    env = child_env()
    if 'BUILDBUDDY_API_KEY' in os.environ:
        env['BUILDBUDDY_API_KEY'] = os.environ['BUILDBUDDY_API_KEY']
    status = subprocess.run([sys.executable, '-I', '-S', str(ROOT / 'tools/ci/before_upload.py'),
                             '--require', 'BUILDBUDDY_API_KEY', str(directory)], env=env).returncode
    if status and directory.exists():
        print('artifact scan: FAIL; artifacts deleted', flush=True)
        for path in directory.iterdir():
            if path.is_dir() and not path.is_symlink():
                shutil.rmtree(path)
            else:
                path.unlink()
    return status


def shard_entrypoint(argv):
    """Keep the runner credential in this supervisor; prove with a scrubbed child."""
    directory = Path(os.environ['BUILDBUDDY_ARTIFACTS_DIRECTORY'])
    try:
        status = subprocess.run([sys.executable, __file__, 'shard-child', *argv],
                                env=child_env()).returncode
    finally:
        scanned = scan(directory)
    return scanned or status


# GitHub side.

def run_request(args, index, names, minutes):
    import remote_gate
    command = ['tools/ci/kani_gate.py', 'shard', '--head', args.head, '--mode', args.mode,
               '--index', str(index), *names]
    return {
        'repo': args.repo, 'branch': args.branch, 'commit_sha': args.head,
        'steps': [{'run': 'python3 ' + shlex.join(command)}],
        'env': {'GIT_REPO_DEFAULT_BRANCH': 'main'},
        'platform_properties': {
            'EstimatedCPU': str(CPUS), 'EstimatedMemory': f'{MEMORY_GB}GB',
            'EstimatedFreeDiskBytes': '50GB', 'container-image': remote_gate.IMAGE,
            'workload-isolation-type': 'firecracker', 'recycle-runner': 'true',
        },
        'timeout': f'{minutes}m',
        'wait_until': 'STARTED',
    }


def invocation_path():
    return Path(os.environ.get('RUNNER_TEMP', '.')) / 'tl3-kani-invocations'


def cancel_recorded(client=None, skip=()):
    import remote_gate
    path = invocation_path()
    if not path.exists():
        return
    client = client or remote_gate.BuildBuddy(os.environ['BUILDBUDDY_API_KEY'])
    for invocation_id in path.read_text().split():
        if invocation_id in skip:
            continue
        try:
            client.call('CancelExecutions', {'invocation_id': invocation_id}, timeout=5)
        except remote_gate.RemoteError as error:
            print(f'::warning::Cancelling {invocation_id} failed: {error}', flush=True)


def check_shard(raw, head, names):
    """Every assigned harness must report, for this head."""
    if raw is None:
        raise KaniError(f'the shard left no {RESULT}')
    try:
        record = json.loads(raw)
    except json.JSONDecodeError as error:
        raise KaniError(f'{RESULT} is not JSON: {error}') from error
    if record.get('schema') != SCHEMA or record.get('head') != head:
        raise KaniError(f'{RESULT} has the wrong schema or head')
    if sorted(record.get('harnesses', [])) != sorted(names):
        raise KaniError(f'{RESULT} names other harnesses than its shard')
    return record


def prove(client, args, names, budgets, sleep=time.sleep):
    """Launch every shard, wait, and collect one result per harness."""
    import remote_gate
    shards = plan(names, budgets, target=PR_TARGET_SECONDS if args.mode == 'pr' else TARGET_SECONDS)
    results, errors, invocations, finished = {}, [], {}, set()
    lock = threading.Lock()
    failed = threading.Event()

    def interrupted(signum, _frame):
        for sig in (signal.SIGINT, signal.SIGTERM):
            signal.signal(sig, signal.SIG_IGN)
        raise SystemExit(128 + signum)

    previous = {sig: signal.signal(sig, interrupted) for sig in (signal.SIGINT, signal.SIGTERM)}
    try:
        deadline = time.monotonic() + (PR_WAIT_MINUTES if args.mode == 'pr' else WAIT_MINUTES) * 60
        for index, shard in enumerate(shards):
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise KaniError('Kani coordinator deadline expired during submission')
            runner_minutes = min(args.timeout, PR_WAIT_MINUTES) if args.mode == 'pr' else args.timeout
            invocation_id = client.call('Run', run_request(args, index, shard, runner_minutes),
                                        timeout=min(600, remaining)).get('invocationId')
            if not invocation_id:
                raise KaniError('BuildBuddy Run returned no invocation id')
            invocations[invocation_id] = shard
            with invocation_path().open('a') as handle:
                handle.write(invocation_id + '\n')
            print(f'Kani shard {index}: {len(shard)} harnesses: '
                  f'https://app.buildbuddy.io/invocation/{invocation_id}', flush=True)

        def follow(invocation_id):
            shard = invocations[invocation_id]
            try:
                invocation = remote_gate.wait(client, invocation_id, deadline, handle_signals=False)
                raw = remote_gate.fetch(client, invocation, RESULT)
                record = check_shard(raw, args.head, shard)
                if record.get('error'):
                    raise KaniError(record['error'])
                reported = record['results']
            except (KaniError, remote_gate.RemoteError) as error:
                with lock:
                    errors.append(f'{invocation_id}: {error}')
                failed.set()
                return
            with lock:
                finished.add(invocation_id)
                for name in shard:
                    results[name] = reported.get(name) or {
                        'name': name, 'verdict': 'MISSING', 'reason': 'no result reported'}
                    results[name]['invocation'] = invocation_id
                if any(not acceptable(results[name]) for name in shard):
                    failed.set()

        # Daemon threads: after a fail-fast cancel, nothing waits for the others.
        threads = [threading.Thread(target=follow, args=(invocation_id,), daemon=True)
                   for invocation_id in invocations]
        for thread in threads:
            thread.start()
        while any(thread.is_alive() for thread in threads):
            if failed.is_set() and args.mode == 'pr':
                # Fail fast: the gate already fails, so free the executors.
                print('Kani: a shard failed; cancelling the others', flush=True)
                break
            if time.monotonic() >= deadline:
                errors.append('Kani coordinator deadline expired')
                break
            sleep(min(5, max(0, deadline - time.monotonic())))
        with lock:
            for invocation_id, shard in invocations.items():
                for name in shard:
                    results.setdefault(name, {'name': name, 'verdict': 'NOT_RUN', 'invocation': invocation_id,
                                              'reason': 'its shard did not report'})
            return results, list(errors)
    finally:
        with lock:
            done = set(finished)
        try:
            cancel_recorded(client, skip=done)
        finally:
            for sig, handler in previous.items():
                signal.signal(sig, handler)


def summarize(mode, head, selection, results, errors):
    lines = [f'Kani ({mode}) at {head[:12]}: {selection}']
    counts = {}
    for result in results.values():
        counts[result['verdict']] = counts.get(result['verdict'], 0) + 1
    if results:
        lines.append(', '.join(f'{count} {name}' for name, count in sorted(counts.items())))
    for error in errors:
        lines.append(f'ERROR: {error}')
    lines.append('| Harness | Verdict | Seconds | Budget | Covers | Peak GB |')
    lines.append('|---|---|---:|---:|---|---:|')
    order = sorted(results.values(), key=lambda r: (r['verdict'] == 'SUCCESSFUL', r['name']))
    for result in order:
        covers = '{}/{}'.format(*result['covers']) if result.get('covers') else ''
        note = ' (warning)' if result.get('warning') else ''
        reason = f' ({result["reason"]})' if result.get('reason') else ''
        lines.append(f'| {result["name"]} | {result["verdict"]}{reason} | {result.get("seconds", "")}{note} '
                     f'| {result.get("warn_after", "")} | {covers} | {result.get("peak_gb", "")} |')
    return '\n'.join(lines)


def write_github(variable, text):
    path = os.environ.get(variable)
    if path:
        with open(path, 'a') as handle:
            handle.write(text)


def gate_main(args):
    import remote_gate
    output = Path(args.output_dir)
    output.mkdir(parents=True, exist_ok=True)
    names = harnesses()
    budgets = load_budgets()
    retired = set(budgets) - set(names)
    if retired:
        raise KaniError('proof inventory lost harnesses; retire their budget entries in the same change: '
                        + ', '.join(sorted(retired)))
    if args.mode == 'pr':
        trigger = triggered(changed_files(args.base, args.head))
        if not trigger:
            selection = 'no change can affect a proof; no harness runs'
            summary = summarize(args.mode, args.head, selection, {}, [])
            print(summary, flush=True)
            write_github('GITHUB_STEP_SUMMARY', summary + '\n')
            (output / 'kani-result.json').write_text(json.dumps(
                {'mode': args.mode, 'head': args.head, 'selection': [], 'results': {}}) + '\n')
            return 0
        missing = [name for name in names if budgets.get(name, {}).get('tier') not in ('pr', 'nightly')]
        if missing:
            raise KaniError('remeasure harnesses and rebuild budgets: ' + ', '.join(missing))
        names = [name for name in names if budgets[name]['tier'] == 'pr']
        selection = f'PR tier: {len(names)} harnesses measured at most {PR_SECONDS}s, because of ' + ', '.join(trigger[:5])
    else:
        selection = f'all {len(names)} harnesses'
    print(f'Kani: {selection}', flush=True)
    client = remote_gate.BuildBuddy(os.environ['BUILDBUDDY_API_KEY'])
    started = time.monotonic()
    try:
        results, errors = prove(client, args, names, budgets)
    except (KaniError, remote_gate.RemoteError) as error:
        results, errors = {}, [str(error)]
    passed = not errors and len(results) == len(names) and all(
        acceptable(result) for result in results.values())
    summary = summarize(args.mode, args.head, selection, results, errors)
    print(summary, flush=True)
    write_github('GITHUB_STEP_SUMMARY', summary + '\n')
    (output / 'kani-summary.md').write_text(summary + '\n')
    (output / 'kani-result.json').write_text(json.dumps(
        {'mode': args.mode, 'head': args.head, 'passed': passed, 'errors': errors,
         'seconds': round(time.monotonic() - started, 1), 'results': results},
        indent=2, sort_keys=True) + '\n')
    print('KANI_RESULT ' + json.dumps({'mode': args.mode, 'head': args.head, 'passed': passed,
                                       'harnesses': len(results), 'errors': len(errors)}), flush=True)
    return 0 if passed else 1


ISSUE_TITLE = 'Nightly Kani run failed on main'


def report(summary, repository, token, run_url, opener=urllib.request.urlopen):
    """Open the regression issue, or comment on the open one."""
    def call(method, path, body=None):
        request = urllib.request.Request(
            f'https://api.github.com/repos/{repository}/{path}', method=method,
            data=None if body is None else json.dumps(body).encode(),
            headers={'Authorization': f'Bearer {token}', 'Accept': 'application/vnd.github+json'})
        with opener(request, timeout=120) as response:
            return json.load(response)

    text = f'The nightly Kani run failed: {run_url}\n\n{summary}\n'
    page = 1
    while True:
        issues = call('GET', f'issues?state=open&per_page=100&creator=github-actions%5Bbot%5D&page={page}')
        for issue in issues:
            if issue.get('title') == ISSUE_TITLE and 'pull_request' not in issue:
                call('POST', f'issues/{issue["number"]}/comments', {'body': text})
                return issue['number']
        if len(issues) < 100:
            break
        page += 1
    return call('POST', 'issues', {'title': ISSUE_TITLE, 'body': text})['number']


def main(argv=None):
    argv = sys.argv[1:] if argv is None else argv
    if argv[:1] == ['shard']:
        return shard_entrypoint(argv[1:])
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='command', required=True)
    shard = sub.add_parser('shard-child')
    shard.add_argument('--head', required=True)
    shard.add_argument('--mode', choices=['pr', 'nightly'], required=True)
    shard.add_argument('--index', type=int, required=True)
    shard.add_argument('harness', nargs='+')
    for mode in ('pr', 'nightly'):
        gate = sub.add_parser(mode)
        gate.add_argument('--base', default='')
        gate.add_argument('--head', required=True)
        gate.add_argument('--branch', required=True)
        gate.add_argument('--repo', default='https://github.com/RobTand/trustladderiii')
        gate.add_argument('--timeout', type=int, default=RUNNER_MINUTES,
                          help='each shard runner\'s limit, in minutes')
        gate.add_argument('--output-dir', default='gate-output')
    builder = sub.add_parser('budgets')
    builder.add_argument('--measurements', required=True)
    builder.add_argument('--output', default=str(BUDGETS))
    sub.add_parser('cancel')
    issue = sub.add_parser('report')
    issue.add_argument('--summary', required=True)
    issue.add_argument('--dry-run', action='store_true')
    args = parser.parse_args(argv)
    if args.command == 'budgets':
        data = json.loads(Path(args.measurements).read_text())
        Path(args.output).write_text(json.dumps(build_budgets(data.get('harnesses', data)),
                                               indent=1, sort_keys=True) + '\n')
        return 0
    if args.command == 'shard-child':
        return shard_main(args)
    if args.command == 'cancel':
        cancel_recorded()
        return 0
    if args.command == 'report':
        path = Path(args.summary)
        summary = path.read_text() if path.exists() else 'The run left no summary; see the run log.'
        if args.dry_run:
            print(f'Would open or update issue: {ISSUE_TITLE}\n\n{summary}', flush=True)
            return 0
        run_url = (f"{os.environ['GITHUB_SERVER_URL']}/{os.environ['GITHUB_REPOSITORY']}"
                   f"/actions/runs/{os.environ['GITHUB_RUN_ID']}")
        number = report(summary, os.environ['GITHUB_REPOSITORY'], os.environ['GH_TOKEN'], run_url)
        print(f'Kani regression reported on issue #{number}', flush=True)
        return 0
    args.mode = args.command
    return gate_main(args)


if __name__ == '__main__':
    sys.exit(main())
