"""Check BuildBuddy's effective allocation for executed test actions.

The API key is loaded at runtime and never printed or passed on a command line.
"""
import argparse
import json
import os
from pathlib import Path
import sys
import urllib.error
import urllib.request

RPC = 'https://app.buildbuddy.io/rpc/BuildBuddyService/GetExecution'


def test_receipts(response, label):
    for action in response.get('execution', []):
        # TestRunner also emits an XML-generation action with the same label.
        if (action.get('targetLabel') != label or action.get('actionMnemonic') != 'TestRunner'
                or not action.get('primaryOutputPath', '').endswith('/test.exited_prematurely')):
            continue
        result = action.get('executeResponse', {}).get('result', {})
        metadata = result.get('executionMetadata', {})
        auxiliary = next((entry for entry in metadata.get('auxiliaryMetadata', [])
                          if entry.get('@type', '').endswith('ExecutionAuxiliaryMetadata')), {})
        scheduling = auxiliary.get('schedulingMetadata', {})
        usage = metadata.get('usageStats', {})
        timeline = usage.get('timeline', {})
        durations = list(map(int, timeline.get('timestamps', [])))[1:]
        cpu = list(map(int, timeline.get('cpuSamples', [])))[1:]
        burst = max((sample / duration for sample, duration in zip(cpu, durations)
                     if duration > 0), default=None)
        yield {
            'action_digest': action.get('actionDigest'),
            'execution_id': action.get('executionId'),
            'shard': (Path(action['primaryOutputPath']).parent.name
                      if Path(action['primaryOutputPath']).parent.name.startswith('shard_') else None),
            'stage': action.get('stage'),
            'status': action.get('status', {}),
            'exit_code': result.get('exitCode', 0),
            'requested': scheduling.get('requestedTaskSize', {}),
            'effective': scheduling.get('taskSize', {}),
            'predicted': scheduling.get('predictedTaskSize'),
            'measured': scheduling.get('measuredTaskSize'),
            'cgroup': scheduling.get('cgroupSettings'),
            'isolation': auxiliary.get('isolationType'),
            'started': metadata.get('executionStartTimestamp'),
            'completed': metadata.get('executionCompletedTimestamp'),
            'cpu_seconds': int(usage['cpuNanos']) / 1e9 if 'cpuNanos' in usage else None,
            'peak_memory_bytes': usage.get('peakMemoryBytes'),
            'sampled_peak_cpu': burst,
        }


def check(receipts, cpu, memory, shards=1):
    if not receipts:
        raise ValueError('no executed test receipts; cached tests do not prove allocation')
    expected = {f'shard_{index}_of_{shards}' for index in range(1, shards + 1)} if shards > 1 else {None}
    if {receipt['shard'] for receipt in receipts} != expected:
        raise ValueError('execution receipts do not cover the expected shards')
    for receipt in receipts:
        if (receipt['stage'] != 'COMPLETED' or receipt['status'].get('code', 0)
                or receipt['exit_code'] or not receipt['started'] or not receipt['completed']):
            raise ValueError('test execution is incomplete or failed')
        for kind in ['requested', 'effective']:
            size = receipt[kind]
            if (int(size.get('estimatedMilliCpu', 0)) != cpu * 1000
                    or int(size.get('estimatedMemoryBytes', 0)) != memory):
                raise ValueError(f'{kind} allocation differs from {cpu} CPUs / {memory} bytes')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('invocation')
    parser.add_argument('label')
    parser.add_argument('--cpu', type=int, required=True)
    parser.add_argument('--shards', type=int, default=1)
    parser.add_argument('--memory-bytes', type=int, required=True)
    args = parser.parse_args()
    if args.cpu <= 0 or args.memory_bytes <= 0 or args.shards <= 0:
        parser.error("CPU, memory and shard count must be positive")
    key = os.environ.get('BUILDBUDDY_API_KEY', '').strip()
    try:
        if not key:
            key = (Path.home() / '.config/buildbuddy/api_key').read_text().strip()
        if not key:
            raise ValueError('no BuildBuddy key available')
        request = urllib.request.Request(
            RPC, data=json.dumps({'executionLookup': {'invocationId': args.invocation},
                                  'inlineExecuteResponse': True}).encode(),
            headers={'x-buildbuddy-api-key': key, 'Content-Type': 'application/json'})
        with urllib.request.urlopen(request, timeout=120) as response:
            receipts = list(test_receipts(json.load(response), args.label))
        print(json.dumps({'invocation': args.invocation, 'label': args.label,
                          'receipts': receipts}, indent=2).replace(key, '[key]'))
        check(receipts, args.cpu, args.memory_bytes, args.shards)
    except (OSError, ValueError, urllib.error.URLError) as error:
        message = str(error).replace(key, '[key]') if key else str(error)
        sys.exit(message)


if __name__ == '__main__':
    main()
