"""Clear only the 80 unserved prerequisite incidents diagnosed in TL3 #810.

Run after deploying the fix. Defaults to printing a plan. Only Rob runs --apply.
Never replays inputs or changes the runtime configuration.
"""

import argparse
import json
from pathlib import Path
import re
import subprocess
import time


SUMMARY = ('For "An incoming update", GitHub could not complete its setup. '
           'Work is waiting for recovery. Other requests can continue.')
OPERATIONS = 'repos/RobTand/trustladderiii/issues/81/comments'
INCIDENTS = frozenset("""
71939.input.0
71941.input.0
71943.input.0
71945.input.0
71947.input.0
71949.input.0
72817.input.0
73692.input.0
75047.input.0
75049.input.0
75053.input.0
75706.input.0
75711.input.0
75713.input.0
75715.input.0
79536.input.0
79539.input.0
79598.input.0
79601.input.0
79604.input.0
79611.input.0
79613.input.0
79615.input.0
79617.input.0
79619.input.0
79622.input.0
79625.input.0
80594.input.0
82715.input.0
82722.input.0
82724.input.0
82726.input.0
82728.input.0
82730.input.0
84066.input.0
84071.input.0
85840.input.0
85842.input.0
85844.input.0
85846.input.0
85848.input.0
85862.input.0
85864.input.0
85866.input.0
85868.input.0
86416.input.0
86420.input.0
86422.input.0
86703.input.0
86708.input.0
86710.input.0
87345.input.0
87349.input.0
87351.input.0
87478.input.0
87480.input.0
87482.input.0
87487.input.0
87489.input.0
87491.input.0
87634.input.0
87638.input.0
87640.input.0
87755.input.0
87758.input.0
87881.input.0
87885.input.0
87887.input.0
87932.input.0
87935.input.0
87971.input.0
87975.input.0
88095.input.0
88097.input.0
88230.input.0
88236.input.0
88238.input.0
88428.input.0
88434.input.0
88436.input.0
""".split())


def plan(status):
    selected = set()
    for line in status.splitlines():
        match = re.search(r' \(clear: clear incident ([0-9]+\.input\.0)\)$', line)
        if match and match[1] in INCIDENTS:
            if line != f'{SUMMARY} (clear: clear incident {match[1]})':
                raise SystemExit(f'Incident {match[1]} changed; inspect it before clearance.')
            selected.add(match[1])
    return sorted(selected, key=lambda incident: int(incident.split('.')[0]))


def run(*command):
    result = subprocess.run(command, capture_output=True, text=True)
    if result.returncode:
        raise SystemExit(result.stderr.strip() or f'{command[0]} failed')
    return result.stdout


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--tl3', default=str(Path.home() / '.local/share/tl3/bin/tl3'))
    parser.add_argument('--config', required=True)
    parser.add_argument('--apply', action='store_true', help='Post clear commands as Rob after deploying the fix.')
    args = parser.parse_args()
    incidents = plan(run(args.tl3, 'status', '--config', args.config))
    for incident in incidents:
        print(f'clear incident {incident}', flush=True)
    print(f'{len(incidents)} diagnosed incidents remain open.', flush=True)
    if not args.apply or not incidents:
        return
    actor = json.loads(run('gh', 'api', 'user'))['login']
    if actor != 'RobTand':
        raise SystemExit('Apply requires RobTand; another actor cannot authorize clearance.')
    for incident in incidents:
        # ClearIncident accepts exactly one command per comment. Stop on every REST error.
        response = json.loads(run('gh', 'api', '--method', 'POST', OPERATIONS,
                                  '-f', f'body=clear incident {incident}'))
        print(response['html_url'], flush=True)
        time.sleep(1.1)


if __name__ == '__main__':
    main()
