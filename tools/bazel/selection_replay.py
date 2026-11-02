"""Audit Bazel input graphs and replay the Gate's historical hash selection.

Run --historical only on Remote Bazel: it checks out both sides of each merge
with each macro version and calls bazel_gate.Workspace.bazel_diff and plan.
For a dependency matrix, capture `bazel query //... --output=xml` before and
after, then pass --before and --after. That mode measures fixed-graph reach.
"""
import argparse
from collections import defaultdict
import json
import math
from pathlib import Path
import subprocess
import sys
import tempfile
import xml.etree.ElementTree as ET

# Also works as a standalone analysis command, without a Bazel Python launcher.
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'ci'))
import bazel_gate  # noqa: E402


class Graph:
    def __init__(self, path):
        self.inputs, self.reverse, self.tests = {}, defaultdict(set), set()
        self.sources = set()
        for item in ET.parse(path).getroot():
            label = bazel_gate.canonical(item.attrib['name'])
            if item.tag == 'rule':
                inputs = {bazel_gate.canonical(child.attrib['name'])
                          for child in item.findall('rule-input')}
                self.inputs[label] = inputs
                for dependency in inputs:
                    self.reverse[dependency].add(label)
                tags = {child.attrib['value'] for child in item.findall("list[@name='tags']/string")}
                if item.attrib['class'].endswith('_test') and 'manual' not in tags:
                    self.tests.add(label)
            elif item.tag == 'source-file':
                self.sources.add(label)
        # //... does not print unexported source-file nodes, but rule-input does.
        self.sources |= {label for label in self.reverse if label.startswith('//')
                         and label not in self.inputs}
        self.used = {label[2:].replace(':', '/') for label in self.sources}
        self.used |= {label.split(':')[0][2:] + '/BUILD.bazel' for label in self.inputs}

    def impacted(self, paths):
        pending = [label for label in self.sources if label[2:].replace(':', '/') in paths]
        found = set(pending)
        while pending:
            for label in self.reverse[pending.pop()] - found:
                found.add(label)
                pending.append(label)
        return found & self.inputs.keys()

    def dependencies(self, label):
        pending, found = [label], set()
        while pending:
            current = pending.pop()
            if current not in found:
                found.add(current)
                pending.extend(self.inputs.get(current, ()))
        return found


def replay(before, after, merges):
    workspace = bazel_gate.Workspace()
    rows = []
    for commit, subject in merges:
        changed, deleted = workspace.changes(commit + '^1', commit)
        row = {'merge': commit, 'subject': subject, 'files': sorted(changed | deleted)}
        for name, graph in [('before', before), ('after', after)]:
            try:
                decision = bazel_gate.plan(changed, deleted, graph.impacted(changed | deleted),
                                           graph.used, graph.tests)
                row[name] = {'selection': decision['selection'], 'selected': decision['selected'],
                             'controller': [label for label in decision['selected']
                                            if label.startswith('//crates/controller:')]}
            except bazel_gate.GateError as error:
                row[name] = {'error': str(error)}
        rows.append(row)
    return rows


def historical(head, count, before_ref):
    """Use the Gate's real bazel-diff hash replay on each historical merge.

    Overlay each macro version on both sides of each merge, so this measures
    the dependency change without counting the macro edit as a full fallback.
    All checkouts and Bazel processes belong on a Remote Bazel runner.
    """
    root = bazel_gate.ROOT
    versions = {
        "before": subprocess.check_output(['git', 'show', before_ref + ':tools/bazel/rust.bzl'],
                                           text=True),
        "after": (root / 'tools/bazel/rust.bzl').read_text(),
    }
    groups = (root / 'workers/src/BUILD.bazel').read_text()
    groups = groups[groups.index('filegroup(\n    name = "prompts",'):]
    groups = groups[:groups.index('filegroup(\n    name = "runtime_files",')]

    class Overlay(bazel_gate.Workspace):
        def __init__(self, version):
            super().__init__()
            self.version = version

        def checkout(self, sha):
            super().checkout(sha)
            (root / 'tools/bazel/rust.bzl').write_text(versions[self.version])
            if self.version == 'after':
                build = root / 'workers/src/BUILD.bazel'
                build.write_text(build.read_text() + '\n' + groups)

    history = subprocess.check_output(
        ['git', 'log', head, '--first-parent', '--merges', f'-{count}', '--format=%H %s'], text=True)
    rows = []
    for commit, subject in (line.split(' ', 1) for line in history.splitlines()):
        row = {'merge': commit, 'subject': subject}
        for version in versions:
            workspace = Overlay(version)
            changed, deleted = workspace.changes(commit + '^1', commit)
            row['files'] = sorted(changed | deleted)
            print(f"REPLAY {subject} {version}", flush=True)
            workspace.checkout(commit)
            used, tests = workspace.used_sources(), workspace.tests()
            impacted = set()
            if not bazel_gate.fallback_files(changed | deleted) and not bazel_gate.needs_no_diff(
                    changed, deleted, used):
                impacted = workspace.bazel_diff(commit + '^1', commit)
            try:
                decision = bazel_gate.plan(changed, deleted, impacted, used, tests)
                row[version] = {'selection': decision['selection'], 'selected': decision['selected'],
                                'controller': [label for label in decision['selected']
                                               if label.startswith('//crates/controller:')]}
            except bazel_gate.GateError as error:
                row[version] = {'error': str(error)}
            print('REPLAY_RESULT ' + json.dumps(row[version]), flush=True)
        rows.append(row)
        Path(tempfile.gettempdir(), 't454-replay-progress.json').write_text(json.dumps(rows))
    return rows


def summary(rows):
    result = {}
    for name in ['before', 'after']:
        result[name] = {}
        for scope in ['selected', 'controller']:
            counts = sorted(len(row[name][scope]) for row in rows if 'error' not in row[name])
            result[name][scope] = {f'p{percent}': counts[math.ceil(len(counts) * percent / 100) - 1]
                                  for percent in [50, 90]} if counts else {}
        result[name]['errors'] = sum('error' in row[name] for row in rows)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--before', type=Path)
    parser.add_argument('--after', type=Path)
    parser.add_argument('--historical', action='store_true')
    parser.add_argument('--before-ref', default='origin/main')
    parser.add_argument('--head', default='origin/main')
    parser.add_argument('--count', type=int, default=20)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    if args.historical:
        rows = historical(args.head, args.count, args.before_ref)
        result = {'method': historical.__doc__, 'summary': summary(rows), 'rows': rows}
        args.output.write_text(json.dumps(result, indent=2) + '\n')
        print(json.dumps(result['summary'], indent=2))
        return
    if not args.before or not args.after:
        parser.error('--before and --after are required for fixed-graph analysis')
    graphs = [Graph(path) for path in [args.before, args.after]]
    history = subprocess.check_output(
        ['git', 'log', args.head, '--first-parent', '--merges', f'-{args.count}',
         '--format=%H %s'], text=True)
    rows = replay(*graphs, [line.split(' ', 1) for line in history.splitlines()])
    matrix = {label: {name: sorted(graph.dependencies(label))
                      for name, graph in zip(['before', 'after'], graphs)}
              for label in sorted(graphs[1].tests) if label.startswith('//crates/controller:')}
    result = {'method': __doc__, 'head': args.head, 'summary': summary(rows),
              'rows': rows, 'matrix': matrix}
    args.output.write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps(result['summary'], indent=2))


if __name__ == '__main__':
    main()
