"""Compare base/head graph health, or record a gain with --write-baseline.

Run this on BuildBuddy. The command checks out both revisions in its runner.
"""
import argparse
import json
import os
from pathlib import Path

import bazel_gate
import bazel_graph_impact

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--base', required=True)
    parser.add_argument('--head', default='HEAD')
    parser.add_argument('--write-baseline', action='store_true')
    args = parser.parse_args()
    workspace = bazel_gate.Workspace()
    head = workspace.run('git', 'rev-parse', args.head).strip()
    workspace.ensure_commit(args.base)
    workspace.ensure_commit(head)
    result = workspace.graph_check(args.base, head, write=args.write_baseline)
    output = os.environ.get('BUILDBUDDY_ARTIFACTS_DIRECTORY')
    if output:
        (Path(output) / 'graph-impact-result.json').write_text(json.dumps(result, indent=2, sort_keys=True) + '\n')
        if args.write_baseline:
            (Path(output) / 'graph-impact-baseline.json').write_bytes((workspace.root / bazel_graph_impact.POLICY).read_bytes())
    print(json.dumps(result, indent=2, sort_keys=True))
    return int(bool(result['errors']))


if __name__ == '__main__':
    raise SystemExit(main())
