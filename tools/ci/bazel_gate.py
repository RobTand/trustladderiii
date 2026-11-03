"""The Bazel merge gate. Runs on a BuildBuddy Remote Bazel runner.

pr mode selects the tests a change can affect, runs them on remote execution
and reports Mergify scopes. main mode runs every test on amd64 and arm64 after
a merge and records whether the merged change's own selection would have
caught each failure. Both modes write gate-result.json to the BuildBuddy
artifacts directory, and the GitHub job reads it back. Any detection failure
is a gate failure, never an empty pass.
"""
import argparse
import base64
from fnmatch import fnmatch
import hashlib
import json
import os
from pathlib import Path
import re
import shlex
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import urllib.request
from urllib.parse import quote, quote_plus
import zipfile

import bazel_generated
import bazel_graph_impact
import rust_gate
from child_env import child_env

ROOT = Path(__file__).resolve().parents[2]
SCHEMA = 'tl3.bazel-gate.v1'
RESULT = 'gate-result.json'

BAZEL_DIFF_URL = 'https://github.com/Tinder/bazel-diff/releases/download/v70.0.2/bazel-diff-rust-linux-amd64'
BAZEL_DIFF_SHA256 = '44226a1d1d5239abc8bc6158de1f886fcbbf58e44479ec178df8c01e7e32102e'

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
    # The Kani manifest writer; only PrismaBuild proof campaigns run it.
    'crates/kernel/run.py',
]
CONFIGS = {'amd64': 'remote', 'arm64': 'remote-arm64'}
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


def diff_commands(executable, root, bazel, base_hashes, head_hashes, impacted, head_edges):
    """One native bazel-diff pin for hashes, dependency edges, selection and blame."""
    def generate(output, edges):
        return [str(executable), 'generate-hashes', '--workspacePath', str(root),
                '--bazelPath', str(bazel), '--excludeExternalTargets', '--includeTargetType',
                '--depEdgesFile', str(edges), str(output)]
    return generate, [str(executable), 'get-impacted-targets',
                      '--workspacePath', str(root), '--bazelPath', str(bazel),
                      '--targetType', 'Rule', '--startingHashes', str(base_hashes),
                      '--finalHashes', str(head_hashes), '--depEdgesFile', str(head_edges),
                      '--output', str(impacted)]


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
    """Mode policy; the caller owns the Bazel process."""

    def __init__(self, mode, quarantine=QUARANTINE, grace=60):
        self.mode, self.quarantine, self.grace = mode, quarantine, min(grace, 60)
        self.failure, self.deadline = None, None
        self.failures, self.observed = set(), set()

    def event(self, event, now):
        identity = event.get('id', {})
        result = identity.get('testResult')
        summary = identity.get('testSummary')
        if result or (summary and event.get('testSummary', {}).get('overallStatus', 'NO_STATUS') != 'NO_STATUS'):
            self.observed.add(canonical((result or summary)['label']))
        reason, label = None, None
        status = event.get('testResult', {}).get('status')
        if result and status in ('FAILED', 'TIMEOUT'):
            label, reason = canonical(result['label']), status
        completed = identity.get('targetCompleted')
        if completed and 'completed' in event and not event['completed'].get('success', False):
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

    def __init__(self, root=ROOT, bazel='bazel', graph_cache=None, mode='main', quarantine=QUARANTINE):
        self.root, self.bazel, self.mode = root, bazel, mode
        self.graph_cache = graph_cache
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

    def prepare_diff(self, base, head):
        """Export once per revision, even when selection falls back to all tests."""
        if getattr(self, '_diff_pair', None) == (base, head):
            return
        executable = Path(tempfile.gettempdir(), 'bazel-diff-v70.0.2-linux-amd64')
        if not executable.exists() or hashlib.sha256(executable.read_bytes()).hexdigest() != BAZEL_DIFF_SHA256:
            with urllib.request.urlopen(BAZEL_DIFF_URL, timeout=120) as response:
                data = response.read()
            if hashlib.sha256(data).hexdigest() != BAZEL_DIFF_SHA256:
                raise GateError('bazel-diff binary checksum mismatch')
            executable.write_bytes(data)
        executable.chmod(0o755)
        self._diff_directory = tempfile.TemporaryDirectory(prefix='tl3-graph-impact-')
        directory = Path(self._diff_directory.name)
        self._hashes = {name: directory / f'{name}-hashes.json' for name in ('base', 'head')}
        self._edges = {name: directory / f'{name}-deps.json' for name in ('base', 'head')}
        self._source_edges = {name: directory / f'{name}-deps-with-sources.json' for name in ('base', 'head')}
        self._impacted = directory / 'impacted.json'
        self._executable = executable
        generate, self._query_impacted = diff_commands(executable, self.root, self.bazel,
            self._hashes['base'], self._hashes['head'], self._impacted, self._edges['head'])
        self._graphs, self._deps, self._typed_hashes, self._crates = {}, {}, {}, {}
        try:
            for name, sha in [('base', base), ('head', head)]:
                files = (self._hashes[name], self._edges[name], self._source_edges[name])
                cached = self.graph_cache.get(sha) if self.graph_cache is not None else None
                if cached:
                    graph, deps, hashes, crates, contents = cached
                    for file, data in zip(files, contents):
                        file.write_bytes(data)
                    self._graphs[name], self._deps[name], self._typed_hashes[name] = graph, deps, hashes
                    self._crates[name] = crates
                else:
                    self.checkout(sha)
                    self.run(*generate(self._hashes[name], self._edges[name]))
                    raw_deps = json.loads(self._edges[name].read_text())
                    sources = bazel_graph_impact.source_labels(json.loads(self._hashes[name].read_text()))
                    if bazel_graph_impact.needs_source_edges(raw_deps, sources):
                        print('bazel-diff omits source edges; supplementing from one streamed Bazel query', flush=True)
                        query = (f'deps({TESTS} except attr(tags, "\\bmanual\\b", //...)) '
                                 'intersect //...:all-targets')
                        stream = self.run(self.bazel, 'query', query, '--output=streamed_jsonproto',
                                          '--keep_going=false')
                        raw_deps = bazel_graph_impact.add_source_edges(raw_deps, stream)
                    self._source_edges[name].write_text(json.dumps(raw_deps, sort_keys=True) + '\n')
                    graph, deps, hashes = bazel_graph_impact.load(
                        self._source_edges[name], self._hashes[name], self.tests(), self.root)
                    self._graphs[name], self._deps[name], self._typed_hashes[name] = graph, deps, hashes
                    self._crates[name] = set(map(canonical, self.query('kind("rust_library rule", //crates/...)')))
                    if self.graph_cache is not None:
                        self.graph_cache[sha] = (graph, deps, hashes, self._crates[name],
                                                 tuple(file.read_bytes() for file in files))
                artifacts = os.environ.get('BUILDBUDDY_ARTIFACTS_DIRECTORY')
                if artifacts:
                    for file in (self._hashes[name], self._edges[name], self._source_edges[name]):
                        shutil.copy2(file, Path(artifacts) / file.name)
        except (ValueError, OSError, KeyError) as error:
            raise GateError(f'graph-impact export failed: {error}') from error
        finally:
            self.checkout(head)
        self._diff_pair = (base, head)

    def bazel_diff(self, base, head):
        self.prepare_diff(base, head)
        self.run(*self._query_impacted)
        return bazel_graph_impact.impacted_labels(self._impacted.read_text())

    def graph_check(self, base, head, write=False):
        self.prepare_diff(base, head)
        old, new = self._graphs['base'], self._graphs['head']
        policy_path = self.root / bazel_graph_impact.POLICY
        previous = bazel_graph_impact.policy_at(self.root, base, bazel_graph_impact.POLICY, None)
        try:
            if write:
                policy = bazel_graph_impact.write_baseline(new, previous)
            else:
                policy = json.loads(policy_path.read_text())
            allowlist = json.loads((self.root / bazel_graph_impact.ALLOWLIST).read_text())
            prior_allowlist = bazel_graph_impact.policy_at(self.root, base, bazel_graph_impact.ALLOWLIST, {})
            changed, deleted = self.changes(base, head)
            changed_tests = bazel_graph_impact.changed_tests(old, new, self._typed_hashes['base'],
                                                           self._typed_hashes['head'], changed | deleted)
            result = bazel_graph_impact.check(old, new, changed_tests, policy, allowlist, prior_allowlist, previous,
                                              record_gains=write)
            if write and not result['errors']:
                policy_path.write_text(json.dumps(policy, indent=2, sort_keys=True) + '\n')
            result['crates'] = bazel_graph_impact.crate_report(self._deps['head'], new['reach'], self._crates['head'])
            result['mermaid'] = {}
            for target in result['explain_targets'][:5]:
                # Explain reads the same exports; it runs no additional Bazel query.
                result['mermaid'][target] = self.run(str(self._executable), 'explain',
                    '--startingHashes', self._hashes['base'], '--finalHashes', self._hashes['head'],
                    '--depEdgesFile', self._source_edges['head'], '--target', target, '--format', 'mermaid')
            print(result['summary'], flush=True)
            return result
        except (ValueError, OSError, KeyError) as error:
            raise GateError(f'graph-impact check failed: {error}') from error

    def watch(self, command, bep):
        """SIGINT the client, then reap it after Bazel cancels remote actions."""
        print('$ ' + shlex.join(map(str, command)), flush=True)
        watcher, stream = FailFast(self.mode, self.quarantine), BepStream(bep)
        self.aborted_early = None
        with subprocess.Popen(list(map(str, command)), cwd=self.root) as process:
            while True:
                for event in stream.read():
                    watcher.event(event, time.monotonic())
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
                watcher.event(event, time.monotonic())
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
            _, observed = self.watch(command, bep)
            self.not_run = sorted(set(targets) - observed)
        return test_statuses(bep)

    def budget_warnings(self, labels):
        """Collect timing warnings from this run, including cached test outputs."""
        warnings = []
        for label in sorted(labels):
            package, _, name = label.removeprefix('//').partition(':')
            outputs = self.root / 'bazel-testlogs' / package / name / 'test.outputs'
            path = outputs / 'budget-warnings.json'
            archive = outputs / 'outputs.zip'
            records = []
            if path.is_file():
                records = json.loads(path.read_text())
            elif archive.is_file():
                with zipfile.ZipFile(archive) as zipped:
                    if path.name in zipped.namelist():
                        records = json.loads(zipped.read(path.name))
            warnings.extend({**record, 'label': label} for record in records)
        return warnings

    def build(self, targets, config, metadata, log_name, extra=()):
        """Build targets; True when every one built."""
        if not targets:
            return True
        patterns = Path(tempfile.gettempdir(), f'{log_name}-build-targets.txt')
        patterns.write_text('\n'.join(targets) + '\n')
        command = [self.bazel, 'build', f'--config={config}', '--config=ci', '--keep_going',
                   f'--target_pattern_file={patterns}', *diagnostic_logs(f'{log_name}-build'), *extra,
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


def exclude_unscanned(directory, diagnostics, secret):
    """Match complete redacted reports, never interpret scanner text as a path."""
    forms = set()
    if secret:
        forms.add(secret)
        for encoded in (base64.b64encode(secret.encode()), base64.urlsafe_b64encode(secret.encode())):
            forms.update((encoded.decode(), encoded.rstrip(b'=').decode()))
        for encoded in (quote(secret), quote(secret, safe=''), quote_plus(secret)):
            forms.update((encoded, re.sub(r'%[0-9A-F]{2}', lambda match: match[0].lower(), encoded)))
    redactions = sorted(forms, key=len, reverse=True)

    def redact(message):
        while any(value in message for value in redactions):
            for value in redactions:
                message = message.replace(value, '')
        return message

    reasons = ('zstd: not scanned (compression.zstd unavailable)',
               'compressed nesting limit, not scanned',
               'gzip member unreadable, not scanned',
               'zstd member unreadable, not scanned',
               'unreadable path, not scanned')
    reports = {}
    for parent, _, filenames in os.walk(directory):
        for filename in filenames:
            path = Path(parent, filename)
            for reason in reasons:
                report = redact(f'{path}: {reason}')
                if report in reports and reports[report] != (path, reason):
                    raise GateError('ambiguous scanner path')
                reports[report] = (path, reason)
    dropped = {}
    for report in diagnostics.splitlines():
        # Mixed secret hits, directory errors and unknown output stay fatal.
        if report not in reports:
            raise GateError('scanner did not report only skipped files')
        path, reason = reports[report]
        dropped.setdefault(path, set()).add(reason)
    if not dropped:
        raise GateError('scanner gave no skipped-file reports')
    for path, reasons in dropped.items():
        size = path.stat().st_size
        path.unlink()
        relative = redact(str(path.relative_to(directory)))
        print(redact(f'artifact scan: dropped {relative!r} ({size} bytes): '
                     + '; '.join(sorted(reasons))), flush=True)


def scan_artifacts(directory):
    """Only the scanner child receives the runner credential, never other tools."""
    if os.environ.get('BBRUN_FINAL_SCAN') == '1':
        print('secret scan deferred to bbrun final scan', flush=True)
        return 0
    env = child_env()
    if 'BUILDBUDDY_API_KEY' in os.environ:
        env['BUILDBUDDY_API_KEY'] = os.environ['BUILDBUDDY_API_KEY']

    def scan():
        result = subprocess.run(
            [sys.executable, '-I', '-S', str(ROOT / 'tools/ci/before_upload.py'),
             '--require', 'BUILDBUDDY_API_KEY', str(directory)], env=env,
            capture_output=True, text=True)
        return result.returncode, result.stdout + result.stderr

    status = 75
    try:
        first, diagnostics = scan()
        if first == 75:
            try:
                exclude_unscanned(directory, diagnostics, env.get('BUILDBUDDY_API_KEY', ''))
            except (GateError, OSError):
                return status
            # Excluding known files never authorizes upload without a clean rescan.
            first, diagnostics = scan()
        status = first or (75 if diagnostics else 0)
        return status
    finally:
        if status and directory.exists():
            print('artifact scan: FAIL; artifacts deleted', flush=True)
            for path in directory.iterdir():
                if path.is_dir() and not path.is_symlink():
                    shutil.rmtree(path)
                else:
                    path.unlink()


def runner_entrypoint(main):
    """Keep the credential in the scan supervisor; run gate logic scrubbed."""
    if sys.argv[1:2] == ['--runner-child']:
        del sys.argv[1]
        return main()
    directory = Path(os.environ['BUILDBUDDY_ARTIFACTS_DIRECTORY'])
    try:
        status = subprocess.run([sys.executable, sys.argv[0], '--runner-child', *sys.argv[1:]],
                                env=child_env()).returncode
    finally:
        scan_status = scan_artifacts(directory)
    return scan_status or status


def gate_pr(workspace, base, head, metadata, mode='pr'):
    workspace.mode = mode
    started = time.monotonic()
    for sha in [base, head]:
        workspace.ensure_commit(sha)
    changed, deleted = workspace.changes(base, head)
    workspace.checkout(head)
    graph = workspace.graph_check(base, head)
    used = workspace.used_sources()
    forced = fallback_files(changed | deleted)
    skip = forced or needs_no_diff(changed, deleted, used)
    impacted = set() if skip else workspace.bazel_diff(base, head)
    workspace.checkout(head)
    tests, rust = workspace.tests(), workspace.rust_tests()
    decision = plan(changed, deleted, impacted, used, tests)
    selected_seconds = time.monotonic() - started
    statuses = workspace.test(decision['selected'], CONFIGS['amd64'], metadata, 'pr')
    warnings = workspace.budget_warnings(statuses)
    aborted = getattr(workspace, 'aborted_early', None)
    not_run = getattr(workspace, 'not_run', sorted(set(decision['selected']) - statuses.keys()))
    verdicts, failed = classify(decision['selected'], statuses, rust)
    inputs = (graph['errors'] if aborted else
              workspace.inputs_check() + workspace.budget_check(base) + graph['errors'])
    if not aborted and not workspace.build(decision['build'], CONFIGS['amd64'], metadata, 'pr'):
        # Reported like stale inputs, so both checks fail.
        inputs.append('bazel build of impacted targets failed: ' + ' '.join(decision['build']))
    aborted = aborted or getattr(workspace, 'aborted_early', None)
    return {**decision, 'verdicts': verdicts, 'failed': failed, 'inputs': inputs, 'budget_warnings': warnings,
            'aborted_early': aborted, 'not_run': not_run,
            'changed': sorted(changed | deleted), 'graph_impact': graph,
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
        warnings = workspace.budget_warnings(statuses)
        verdicts, failed = classify(everything, statuses, rust)
        failures = failed['rust'] + failed['python']
        architectures[architecture] = {
            'verdicts': verdicts, 'failed': failed,
            'missed': sorted(set(failures) - set(selection['selected'])), 'budget_warnings': warnings,
            'seconds': round(time.monotonic() - begun, 1)}
    return {'selection': selection['selection'], 'reason': selection['reason'],
            'selected': selection['selected'], 'tests': len(everything),
            'architectures': architectures,
            'budget_warnings': [{**warning, 'architecture': arch}
                                for arch, info in architectures.items() for warning in info['budget_warnings']],
            'inputs': workspace.inputs_check(),
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
    sys.exit(runner_entrypoint(main))
