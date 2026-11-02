"""The Bazel merge gate. Runs on a BuildBuddy Remote Bazel runner.

pr mode selects the tests a change can affect, runs them on remote execution
and reports Mergify scopes. main mode runs every test on amd64 and arm64 after
a merge and records whether the merged change's own selection would have
caught each failure. Both modes write gate-result.json to the BuildBuddy
artifacts directory, and the GitHub job reads it back. Any detection failure
is a gate failure, never an empty pass.
"""
import argparse
from fnmatch import fnmatch
import hashlib
import json
import os
from pathlib import Path
import shlex
import signal
import subprocess
import sys
import tempfile
import time
import urllib.request
import zipfile

import bazel_generated
import rust_gate
from child_env import child_env

ROOT = Path(__file__).resolve().parents[2]
SCHEMA = 'tl3.bazel-gate.v1'
RESULT = 'gate-result.json'

BAZEL_DIFF_URL = 'https://github.com/Tinder/bazel-diff/releases/download/v42.0.0/bazel-diff_deploy.jar'
BAZEL_DIFF_SHA256 = '0170b70fe2f24477ab056c11f5c3240d8b45a1dca5ab73ad252c096679c5500c'

# A change to any of these can move every target: test everything.
FALLBACK = [
    'MODULE.bazel', 'MODULE.bazel.lock', '.bazelrc', '.bazelversion', '.bazelignore',
    'BUILD.bazel', 'Cargo.toml', 'Cargo.lock', 'crates/*/Cargo.toml', 'crates/*/build.rs',
    '**/uv.lock', '**/pyproject.toml', 'tools/bazel/*', 'rust-toolchain', 'rust-toolchain.toml',
    '.python-version', '.github/workflows/*', 'tools/ci/bazel_gate.py', 'tools/ci/remote_gate.py',
    'gazelle_python.yaml',
]
# Files no build or test reads. Only these may change without selecting a target.
IRRELEVANT = [
    'docs/*', '*.md', 'assets/*', 'spikes/*', 'LICENSE', 'LICENSE.*', '.mergify.yml',
    '.github/*.md', '.github/ISSUE_TEMPLATE/*', '.github/CODEOWNERS',
]
CONFIGS = {'amd64': 'remote', 'arm64': 'remote-arm64'}
# Written by tools/bazel/rust_runner.py when every test passed but a budget overran.
BUDGET_MARKER = 'budget-overrun.txt'
# .bazelrc's default names; test and build runs prefix their own.
DIAGNOSTICS = ['bazel-remote-grpc.log', 'execution_log.binpb.zst']
TESTS = 'tests(//...)'
# No quarantined targets today. A future table supplies canonical labels here.
QUARANTINE = frozenset()


class GateError(RuntimeError):
    """Detection could not establish what to test. The gate fails."""


# fnmatch's * also crosses '/', so 'docs/*' covers the whole tree. FALLBACK
# patterns without '/' name root files; IRRELEVANT ones match any basename.
def fallback_files(changed):
    return sorted(path for path in changed if any(
        fnmatch(path, pattern) if '/' in pattern else path == pattern for pattern in FALLBACK))


def irrelevant(path):
    return any(fnmatch(path, pattern) if '/' in pattern else fnmatch(Path(path).name, pattern)
               for pattern in IRRELEVANT)


def build_graph(path):
    return Path(path).name in ('BUILD.bazel', 'BUILD') or path.endswith('.bzl')


def needs_no_diff(changed, deleted, used_sources):
    """Only unread, allowlisted files changed; bazel-diff has nothing to find."""
    return not deleted and all(irrelevant(path) and path not in used_sources for path in changed)


def diff_commands(java, jar, root, bazel, base_hashes, head_hashes, impacted):
    """bazel-diff v42 command lines: a hash generator, then the impacted query."""
    def generate(output):
        return [str(java), '-jar', str(jar), 'generate-hashes', '--workspacePath', str(root),
                '--bazelPath', str(bazel), '--excludeExternalTargets', '--includeTargetType',
                str(output)]
    return generate, [str(java), '-jar', str(jar), 'get-impacted-targets',
                      '--workspacePath', str(root), '--bazelPath', str(bazel),
                      '--targetType', 'Rule', '--startingHashes', str(base_hashes),
                      '--finalHashes', str(head_hashes), '--output', str(impacted)]


def plan(changed, deleted, impacted, used_sources, tests):
    """Decide what to test. Pure; every input comes from git or Bazel."""
    if not changed and not deleted:
        raise GateError('no changed files between base and head; refusing to pass an empty diff')
    forced = fallback_files(changed | deleted)
    if forced:
        return {'selection': 'fallback', 'reason': forced, 'selected': sorted(tests),
                'build': ['//...'], 'scopes': [], 'all_scopes': True}
    unmapped = sorted(path for path in changed - deleted
                      if path not in used_sources and not irrelevant(path))
    if unmapped:
        raise GateError('changed files that no Bazel target reads: ' + ', '.join(unmapped)
                        + '. Declare them as target inputs (docs/bazel.md), or add the pattern '
                        'to IRRELEVANT in tools/ci/bazel_gate.py if nothing can read them.')
    relevant = [path for path in changed | deleted if not irrelevant(path)]
    if not impacted and relevant and all(build_graph(path) for path in relevant):
        # A BUILD or .bzl edit that changes no rule hash (formatting, comments): test everything.
        return {'selection': 'fallback', 'reason': sorted(relevant), 'selected': sorted(tests),
                'build': ['//...'], 'scopes': [], 'all_scopes': True}
    if not impacted and relevant:
        raise GateError('bazel-diff found no impacted targets for build-relevant changes')
    selected = sorted(set(impacted) & set(tests))
    scopes = sorted({label.split(':', 1)[0] for label in impacted if label.startswith('//')})
    selection = 'impacted' if impacted else 'irrelevant'
    # Impacted targets no test depends on would otherwise never be built.
    build = sorted(set(impacted) - set(tests))
    return {'selection': selection, 'reason': [], 'selected': selected, 'build': build,
            'scopes': scopes, 'all_scopes': False}


def classify(selected, statuses, rust):
    """Split test outcomes into the Rust and Python checks. Missing status fails."""
    failed = {'rust': [], 'python': []}
    for label in selected:
        if statuses.get(label) != 'PASSED':
            failed['rust' if label in rust else 'python'].append(label)
    ran = {'rust': any(label in rust for label in selected),
           'python': any(label not in rust for label in selected)}
    verdicts = {language: 'fail' if failed[language] else 'pass' if ran[language] else 'skipped'
                for language in failed}
    return verdicts, failed


def test_statuses(bep_path):
    statuses = {}
    if not bep_path.exists():
        return statuses
    for line in bep_path.read_text().splitlines():
        event = json.loads(line)
        summary = event.get('id', {}).get('testSummary')
        if summary:
            statuses[summary['label']] = event.get('testSummary', {}).get('overallStatus', 'NO_STATUS')
    return statuses


def canonical(label):
    return label[1:] if label.startswith('@//') else label.removeprefix('@@')


class BepStream:
    """Read new, complete JSON lines without retaining or exposing raw events."""

    def __init__(self, path):
        self.path, self.offset, self.pending = path, 0, b''

    def read(self):
        if not self.path.exists():
            return []
        with self.path.open('rb') as stream:
            stream.seek(self.offset)
            data = stream.read()
            self.offset = stream.tell()
        lines = (self.pending + data).split(b'\n')
        self.pending = lines.pop()
        return [json.loads(line) for line in lines if line.strip()]


class FailFast:
    """Mode policy; the caller owns the process and the budget-marker lookup."""

    def __init__(self, mode, quarantine=QUARANTINE, grace=60):
        self.mode, self.quarantine, self.grace = mode, quarantine, min(grace, 60)
        self.failure, self.deadline = None, None
        self.failures, self.observed = set(), set()

    def event(self, event, now, budget_only):
        identity = event.get('id', {})
        result = identity.get('testResult')
        summary = identity.get('testSummary')
        if result or (summary and event.get('testSummary', {}).get('overallStatus') != 'NO_STATUS'):
            self.observed.add(canonical((result or summary)['label']))
        reason, label = None, None
        if result and event.get('testResult', {}).get('status') == 'FAILED':
            label, reason = canonical(result['label']), 'FAILED'
            # BEP names outputs before remote downloads finish. Looking only
            # at bazel-testlogs races the budget marker's materialization.
            outputs = event['testResult'].get('testActionOutput', [])
            if any(Path(output.get('name', '')).name == BUDGET_MARKER for output in outputs) or label in budget_only([label]):
                return
        completed = identity.get('targetCompleted')
        if completed and event.get('completed', {}).get('success') is False:
            label, reason = canonical(completed['label']), 'BUILD_FAILURE'
        if not label or label in self.quarantine or self.mode == 'main':
            return
        self.failures.add(label)
        if self.failure is None:
            self.failure = {'label': label, 'reason': reason}
            self.deadline = now + (self.grace if self.mode == 'pr' else 0)

    def should_cancel(self, now):
        return self.deadline is not None and now >= self.deadline


class Workspace:
    """Git and Bazel on the runner. Every failure raises GateError."""

    def __init__(self, root=ROOT, bazel='bazel', mode='main', quarantine=QUARANTINE):
        self.root, self.bazel, self.mode = root, bazel, mode
        self.quarantine = quarantine
        self.aborted_early, self.not_run = None, []

    def run(self, *command, capture=True, check=True):
        print('$ ' + shlex.join(map(str, command)), flush=True)
        result = subprocess.run(list(map(str, command)), cwd=self.root, text=True,
                                stdout=subprocess.PIPE if capture else None,
                                env=None if command[0] == self.bazel else child_env())
        if check and result.returncode:
            raise GateError(f'{shlex.join(map(str, command))} exited {result.returncode}')
        return result.stdout if capture else result.returncode

    def ensure_commit(self, sha):
        if not sha:
            raise GateError('empty base or head; the queue-aware refs were unavailable')
        present = subprocess.run(['git', 'cat-file', '-e', f'{sha}^{{commit}}'], cwd=self.root,
                                 stderr=subprocess.DEVNULL, env=child_env())
        if present.returncode:
            self.run('git', 'fetch', '--quiet', '--no-tags', 'origin', sha)

    def changes(self, base, head):
        output = self.run('git', 'diff', '--name-status', '--no-renames', '-z', base, head)
        fields = output.split('\0')
        changed, deleted = set(), set()
        for status, path in zip(fields[0::2], fields[1::2]):
            (deleted if status.startswith('D') else changed).add(path)
        return changed, deleted

    def checkout(self, sha):
        self.run('git', 'checkout', '--quiet', '--force', '--end-of-options', sha, '--')

    def query(self, expression):
        return self.run(self.bazel, 'query', expression, '--output=label', '--keep_going=false').split()

    def tests(self):
        """Runnable tests; the manual rust_test binaries run only through rust_runner."""
        return set(map(canonical, self.query(f'{TESTS} except attr(tags, "\\bmanual\\b", //...)')))

    def rust_tests(self):
        return set(map(canonical, self.query(f'attr(tags, "\\brust\\b", {TESTS})')))

    def used_sources(self):
        # BUILD and .bzl files define targets rather than being read by one; Bazel still reads them.
        labels = self.query('kind("source file", deps(//...))') + self.query('buildfiles(//...)')
        return {label.removeprefix('//').replace(':', '/').lstrip('/') for label in map(canonical, labels)
                if label.startswith('//')}

    def bazel_diff(self, base, head):
        jar = Path(tempfile.gettempdir(), 'bazel-diff-v42.0.0.jar')
        if not jar.exists() or hashlib.sha256(jar.read_bytes()).hexdigest() != BAZEL_DIFF_SHA256:
            with urllib.request.urlopen(BAZEL_DIFF_URL, timeout=120) as response:
                data = response.read()
            if hashlib.sha256(data).hexdigest() != BAZEL_DIFF_SHA256:
                raise GateError('bazel-diff jar checksum mismatch')
            jar.write_bytes(data)
        java = Path(self.run(self.bazel, 'info', 'java-home').strip(), 'bin/java')
        hashes = {name: Path(tempfile.gettempdir(), f'{name}-hashes.json') for name in ['base', 'head']}
        impacted = Path(tempfile.gettempdir(), 'impacted.txt')
        impacted.unlink(missing_ok=True)
        generate, query = diff_commands(java, jar, self.root, self.bazel,
                                        hashes['base'], hashes['head'], impacted)
        for name, sha in [('base', base), ('head', head)]:
            self.checkout(sha)
            self.run(*generate(hashes[name]))
        self.run(*query)
        return {canonical(line) for line in impacted.read_text().split() if line}

    def watch(self, command, bep):
        """SIGINT the client, then reap it after Bazel cancels remote actions."""
        print('$ ' + shlex.join(map(str, command)), flush=True)
        watcher, stream = FailFast(self.mode, self.quarantine), BepStream(bep)
        self.aborted_early = None
        with subprocess.Popen(list(map(str, command)), cwd=self.root) as process:
            while True:
                for event in stream.read():
                    watcher.event(event, time.monotonic(), self.budget_only)
                if process.poll() is not None:
                    break
                if watcher.should_cancel(time.monotonic()):
                    try:
                        process.send_signal(signal.SIGINT)
                    except ProcessLookupError:
                        break
                    self.aborted_early = watcher.failure
                    print('Aborting Bazel: ' + json.dumps(self.aborted_early), flush=True)
                    try:
                        process.wait(timeout=30)
                    except subprocess.TimeoutExpired:
                        # Bazel's second interrupt stops actions that ignored the first.
                        process.send_signal(signal.SIGINT)
                        process.wait(timeout=30)
                    break
                time.sleep(.1)
            # The last events and summaries may arrive during SIGINT shutdown.
            for event in stream.read():
                watcher.event(event, time.monotonic(), self.budget_only)
        return process.returncode, watcher.observed

    def test(self, targets, config, metadata, log_name, extra=()):
        """Run targets; return per-label status. Build failures leave labels without status."""
        if not targets:
            return {}
        patterns = Path(tempfile.gettempdir(), f'{log_name}-targets.txt')
        patterns.write_text('\n'.join(targets) + '\n')
        bep = Path(tempfile.gettempdir(), f'{log_name}-bep.json')
        bep.unlink(missing_ok=True)
        command = [self.bazel, 'test', f'--config={config}', '--config=ci', '--keep_going',
                   f'--target_pattern_file={patterns}', f'--build_event_json_file={bep}',
                   *diagnostic_logs(log_name), *extra,
                   *[f'--build_metadata={key}={value}' for key, value in metadata.items()]]
        if self.mode == 'main':
            self.run(*command, capture=False, check=False)
            self.not_run = []
        else:
            # Each undeclared output must be named in BEP so a budget marker
            # can be recognized without downloading an outputs.zip first.
            command.append('--nozip_undeclared_test_outputs')
            _, observed = self.watch(command, bep)
            self.not_run = sorted(set(targets) - observed)
        return test_statuses(bep)

    def budget_only(self, labels):
        """Failed labels whose runner reported only a time-budget overrun."""
        found = set()
        for label in labels:
            package, _, name = label.removeprefix('//').partition(':')
            outputs = self.root / 'bazel-testlogs' / package / name / 'test.outputs'
            archive = outputs / 'outputs.zip'
            if (outputs / BUDGET_MARKER).is_file():
                found.add(label)
            elif archive.is_file():
                with zipfile.ZipFile(archive) as zipped:
                    if BUDGET_MARKER in zipped.namelist():
                        found.add(label)
        return found

    def build(self, targets, config, metadata, log_name):
        """Build targets; True when every one built."""
        if not targets:
            return True
        patterns = Path(tempfile.gettempdir(), f'{log_name}-build-targets.txt')
        patterns.write_text('\n'.join(targets) + '\n')
        command = [self.bazel, 'build', f'--config={config}', '--config=ci', '--keep_going',
                   f'--target_pattern_file={patterns}', *diagnostic_logs(f'{log_name}-build'),
                   *[f'--build_metadata={key}={value}' for key, value in metadata.items()]]
        if self.mode == 'main':
            return not self.run(*command, capture=False, check=False)
        bep = Path(tempfile.gettempdir(), f'{log_name}-build-bep.json')
        bep.unlink(missing_ok=True)
        code, _ = self.watch([*command, f'--build_event_json_file={bep}'], bep)
        return code == 0

    def budget_check(self, base):
        """Light checks refused PRs that grow test budgets; so does this gate."""
        policy = 'tools/ci/rust-test-budget.json'
        current = json.loads((self.root / policy).read_text())
        previous = subprocess.run(['git', 'show', f'{base}:{policy}'], cwd=self.root,
                                  capture_output=True, text=True, env=child_env())
        try:
            rust_gate.check_budget({}, current, json.loads(previous.stdout) if not previous.returncode
                                   else current)
        except RuntimeError as error:
            return [str(error)]
        return []

    def inputs_check(self):
        """Generated inputs and test parity on the head tree, using Bazel's own cargo."""
        shim = Path(tempfile.gettempdir(), 'cargo')
        shim.write_text(f'#!/bin/sh\nexec {shlex.quote(self.bazel)} run --noshow_progress '
                        '@rules_rust//tools/upstream_wrapper:cargo -- "$@"\n')
        shim.chmod(0o755)
        os.environ['CARGO'] = str(shim)
        problems = []
        for command in [[sys.executable, 'tools/ci/bazel_parity.py', 'static', '--bazel', self.bazel],
                        *bazel_generated.commands(self.bazel)]:
            if self.run(*command, capture=False, check=False):
                problems.append(shlex.join(map(str, command)))
        return problems


def diagnostic_logs(prefix):
    """Per-invocation names, so later helper invocations can't overwrite these logs."""
    return [f'--remote_grpc_log={prefix}-{DIAGNOSTICS[0]}',
            f'--execution_log_compact_file={prefix}-{DIAGNOSTICS[1]}']


def write_result(result, directory, root=ROOT):
    directory.mkdir(parents=True, exist_ok=True)
    (directory / RESULT).write_text(json.dumps(result, indent=2, sort_keys=True) + '\n')
    for suffix in DIAGNOSTICS:
        for log in root.glob(f'*{suffix}'):
            log.replace(directory / log.name)


def rerun_budget_only(workspace, labels, statuses, config, metadata, log_name):
    """Rerun once the targets that failed only a time budget. Real failures are never retried."""
    failed = [label for label in labels if statuses.get(label) != 'PASSED']
    budget = sorted(workspace.budget_only(failed))
    if not budget:
        return statuses, {}
    again = workspace.test(budget, config, metadata, f'{log_name}-budget-rerun',
                           extra=['--nocache_test_results'])
    reruns = {label: again.get(label, 'NO_STATUS') for label in budget}
    return {**statuses, **reruns}, reruns


def gate_pr(workspace, base, head, metadata, mode='pr'):
    workspace.mode = mode
    started = time.monotonic()
    for sha in [base, head]:
        workspace.ensure_commit(sha)
    changed, deleted = workspace.changes(base, head)
    workspace.checkout(head)
    used = workspace.used_sources()
    forced = fallback_files(changed | deleted)
    skip = forced or needs_no_diff(changed, deleted, used)
    impacted = set() if skip else workspace.bazel_diff(base, head)
    workspace.checkout(head)
    tests, rust = workspace.tests(), workspace.rust_tests()
    decision = plan(changed, deleted, impacted, used, tests)
    selected_seconds = time.monotonic() - started
    statuses = workspace.test(decision['selected'], CONFIGS['amd64'], metadata, 'pr')
    aborted = getattr(workspace, 'aborted_early', None)
    not_run = getattr(workspace, 'not_run', sorted(set(decision['selected']) - statuses.keys()))
    reruns = {}
    if not aborted:
        statuses, reruns = rerun_budget_only(workspace, decision['selected'], statuses, CONFIGS['amd64'],
                                             metadata, 'pr')
        aborted = getattr(workspace, 'aborted_early', None)
    verdicts, failed = classify(decision['selected'], statuses, rust)
    inputs = [] if aborted else workspace.inputs_check() + workspace.budget_check(base)
    if not aborted and not workspace.build(decision['build'], CONFIGS['amd64'], metadata, 'pr'):
        # Reported like stale inputs, so both checks fail.
        inputs.append('bazel build of impacted targets failed: ' + ' '.join(decision['build']))
    aborted = aborted or getattr(workspace, 'aborted_early', None)
    return {**decision, 'verdicts': verdicts, 'failed': failed, 'inputs': inputs, 'budget_reruns': reruns,
            'aborted_early': aborted, 'not_run': not_run,
            'changed': sorted(changed | deleted),
            'durations': {'selection': round(selected_seconds, 1),
                          'total': round(time.monotonic() - started, 1)}}


def gate_main(workspace, base, head, metadata):
    """Full run after a merge, on both architectures, plus what selection would have run."""
    workspace.mode = 'main'
    started = time.monotonic()
    for sha in [base, head]:
        workspace.ensure_commit(sha)
    changed, deleted = workspace.changes(base, head)
    selection = None
    try:
        forced = fallback_files(changed | deleted)
        impacted = set() if forced else workspace.bazel_diff(base, head)
        workspace.checkout(head)
        tests = workspace.tests()
        selection = plan(changed, deleted, impacted, workspace.used_sources(), tests)
    except GateError as error:
        workspace.checkout(head)
        tests = workspace.tests()
        selection = {'selection': 'error', 'reason': [str(error)], 'selected': sorted(tests)}
    rust = workspace.rust_tests()
    everything = sorted(tests)
    architectures = {}
    for architecture, config in CONFIGS.items():
        begun = time.monotonic()
        statuses = workspace.test(everything, config, metadata, f'main-{architecture}')
        statuses, reruns = rerun_budget_only(workspace, everything, statuses, config, metadata,
                                             f'main-{architecture}')
        verdicts, failed = classify(everything, statuses, rust)
        failures = failed['rust'] + failed['python']
        architectures[architecture] = {
            'verdicts': verdicts, 'failed': failed,
            'missed': sorted(set(failures) - set(selection['selected'])), 'budget_reruns': reruns,
            'seconds': round(time.monotonic() - begun, 1)}
    return {'selection': selection['selection'], 'reason': selection['reason'],
            'selected': selection['selected'], 'tests': len(everything),
            'architectures': architectures, 'inputs': workspace.inputs_check(),
            'changed': sorted(changed | deleted),
            'durations': {'total': round(time.monotonic() - started, 1)}}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('mode', choices=['pr', 'batch-draft', 'main'])
    parser.add_argument('--base', required=True)
    parser.add_argument('--head', required=True)
    parser.add_argument('--branch', default='')
    args = parser.parse_args()
    output = Path(os.environ.get('BUILDBUDDY_ARTIFACTS_DIRECTORY', ROOT / 'gate-artifacts'))
    metadata = {'COMMIT_SHA': args.head, 'BRANCH_NAME': args.branch}
    result = {'schema': SCHEMA, 'mode': args.mode, 'base': args.base, 'head': args.head}
    try:
        workspace = Workspace(mode=args.mode)
        if args.mode == 'main':
            result.update(gate_main(workspace, args.base, args.head, metadata))
        else:
            result.update(gate_pr(workspace, args.base, args.head, metadata, mode=args.mode))
        result['error'] = None
    except GateError as error:
        result['error'] = str(error)
    write_result(result, output)
    print(json.dumps({key: result.get(key) for key in ['selection', 'error', 'verdicts', 'inputs']}))
    return 0 if passed(result) else 1


def passed(result):
    if result.get('error') or result.get('inputs') or result.get('aborted_early'):
        return False
    if result['mode'] == 'main':
        return all(not any(arch['failed'].values()) for arch in result['architectures'].values())
    return all(verdict != 'fail' for verdict in result['verdicts'].values())


if __name__ == '__main__':
    sys.exit(main())
