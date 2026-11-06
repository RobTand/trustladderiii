"""Compare Rust role output with the current Python schema transforms, without a model.

Run remotely: python3 crates/contracts/tools/render_parity.py. Only the named
pure AST bodies execute, so provider imports and runtime model/tool calls are
excluded. The compared Python behavior remains owned by its production sources.
"""
import ast
from copy import deepcopy
import json
from pathlib import Path
import subprocess
from types import SimpleNamespace

ROOT = Path(__file__).resolve().parents[3]
WORKERS = ROOT / 'workers/src/tl3_workers'


def tree(path):
    return ast.parse(path.read_text())


def execute(nodes, namespace):
    module = ast.Module(body=nodes, type_ignores=[])
    exec(compile(ast.fix_missing_locations(module), '<production schema transform>', 'exec'), namespace)


def schema(name):
    return json.loads((WORKERS / 'schemas' / (name.removesuffix('.json') + '.json')).read_text())


def assigned(node, name):
    return isinstance(node, ast.Assign) and any(isinstance(target, ast.Name) and target.id == name
                                              for target in node.targets)


def role_fields(role):
    for node in ast.walk(tree(WORKERS / 'roles' / (role + '.py'))):
        if isinstance(node, ast.keyword) and node.arg == 'code_only_fields':
            return ast.literal_eval(node.value)
    return ()


def main():
    result = subprocess.run(['bazel', 'run', '--config=remote',
                             '//crates/contracts:example_role-vectors'], cwd=ROOT,
                            check=True, stdout=subprocess.PIPE, text=True)
    rows = json.loads(result.stdout)
    contracts = tree(WORKERS / 'contracts.py')
    namespace = {'deepcopy': deepcopy, 'schema': schema}
    execute([node for node in contracts.body if isinstance(node, ast.FunctionDef)
             and node.name in {'_refs', 'model_facing'}], namespace)
    runtime = tree(WORKERS / 'runtime.py')
    only_json = next(ast.literal_eval(node.value) for node in runtime.body if assigned(node, 'ONLY_JSON'))
    planner = tree(WORKERS / 'roles/planner.py')
    exploration = next(ast.literal_eval(node.value) for node in planner.body
                       if assigned(node, 'EXPLORATION_DECLARATION'))
    build = next(node for node in runtime.body if isinstance(node, ast.FunctionDef)
                 and node.name == 'build_agent')
    # Execute the production role-specific hiding loop, ending before prompt append.
    begin = next(i for i, node in enumerate(build.body) if isinstance(node, ast.For)
                 and isinstance(node.target, ast.Name) and node.target.id == 'variant')
    hiding = build.body[begin:begin + 2]
    maker = next(node for node in tree(WORKERS / 'roles/maker.py').body
                 if isinstance(node, ast.FunctionDef) and node.name == 'run')
    begin = next(i for i, node in enumerate(maker.body) if assigned(node, 'proposal'))
    end = next(i for i, node in enumerate(maker.body[begin:], begin) if assigned(node, 'control'))
    count = 0
    for row in rows:
        role = row['role']
        scope = {**namespace, 'proposal': schema(role + '-reply'),
                 'role': SimpleNamespace(code_only_fields=role_fields(role))}
        if role == 'maker':
            execute(maker.body[begin:end], scope)
        if role == 'planner':
            scope['proposal'] = {**exploration, **scope['proposal']}
        execute(hiding, scope)
        expected = namespace['model_facing'](scope['proposal'])
        assert row['model_schema'] == expected, role + ' model-facing schema changed'
        assert row['guidance'] == only_json, role + ' JSON guidance changed'
        count += 1
    assert count == 32, count
    print(f'C0_RENDER_PARITY: {count} role/options cases passed; no model calls')


if __name__ == '__main__':
    main()
