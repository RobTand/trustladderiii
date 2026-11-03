"""Prove maker definitions and fixture-consumer definitions are unchanged.

Usage: python compare_ast.py BASE_COMMIT
This reads Git and Python syntax trees, without importing tests.
"""
import argparse
import ast
from collections import Counter
import json
from pathlib import Path
import subprocess


def definitions(sources):
    return Counter(ast.dump(n) for source in sources for n in ast.parse(source).body
                   if isinstance(n, (ast.FunctionDef, ast.ClassDef, ast.Assign)))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('base')
    args = parser.parse_args()

    def original(name):
        return subprocess.check_output(['git', 'show', f'{args.base}:workers/tests/{name}.py'], text=True)

    before = original('test_maker')
    after = [p.read_text() for p in Path('workers/tests').glob('test_maker*.py')
             if p.name != 'test_maker_budget.py']
    support = Path('workers/tests/message_invariants.py').read_text()
    old = definitions([before])
    new = definitions(after + [support]) - definitions([original('message_invariants')])
    if old != new:
        raise SystemExit(json.dumps({'missing': list((old - new).elements()), 'added': list((new - old).elements())}))
    consumers = ['test_prompt_cache', 'test_blocked', 'test_replies', 'test_turns', 'test_large_files', 'test_diff_tool']
    for name in consumers:
        assert definitions([original(name)]) == definitions([Path(f'workers/tests/{name}.py').read_text()]), name
    print(json.dumps({'equal': True, 'definitions': sum(old.values()), 'test_functions':
        sum(isinstance(n, ast.FunctionDef) and n.name.startswith('test_') for n in ast.parse(before).body),
        'unchanged_consumers': consumers}, indent=2))


if __name__ == '__main__':
    main()
