"""Check real adapter-suite timing markers; run inside an admitted PB action."""
import argparse
import json

from rust_gate import execute_binary


REQUIRED = {
    'tl3-controller/prismabuild::oversized_planner_reply_reaches_plan_feedback',
    'tl3-controller/prismabuild::result_budget_rejects_repeated_checks',
    'tl3-controller/prismabuild::retirement_continues_after_corrupt_node',
}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('binary')
    parser.add_argument('--repeats', type=int, default=1)
    args = parser.parse_args()
    if args.repeats < 1:
        parser.error('--repeats must be positive')
    for repeat in range(args.repeats):
        timings = execute_binary({'name': 'tl3-controller/prismabuild',
                                  'command': [args.binary]}, custom=True)
        print('ADAPTER_TIMINGS ' + json.dumps({'repeat': repeat, 'seconds': timings}), flush=True)
        missing = REQUIRED - timings.keys()
        assert not missing, f'Adapter scenarios must report their own timing markers: {sorted(missing)}'


if __name__ == '__main__':
    main()
