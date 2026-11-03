"""Generate the Rust text policy from #302's authoritative Python definitions.

Run from any directory with Python 3.11+: python redaction_policy.py [--check].
No worker module is executed; only literal policy expressions are read.
"""
import ast
import hashlib
import json
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[3]
SOURCE = ROOT / 'workers/src/tl3_workers/tracing.py'
OUTPUT = ROOT / 'crates/controller/src/redaction/policy.json'


class Pattern:
    def __init__(self, value):
        self.value = value


def policy():
    names = {}

    def value(node):
        if isinstance(node, ast.Constant):
            return node.value
        if isinstance(node, (ast.Tuple, ast.List, ast.Set)):
            result = [value(item) for item in node.elts]
            return sorted(result) if isinstance(node, ast.Set) else result
        if isinstance(node, ast.Name):
            return node.id if node.id.startswith('check_') else names[node.id]
        if isinstance(node, ast.BinOp) and isinstance(node.op, ast.Add):
            return value(node.left) + value(node.right)
        if isinstance(node, ast.BinOp) and isinstance(node.op, ast.Mult):
            return value(node.left) * value(node.right)
        if (isinstance(node, ast.Call) and isinstance(node.func, ast.Attribute)
                and isinstance(node.func.value, ast.Name) and node.func.value.id == 're'
                and node.func.attr == 'compile' and len(node.args) == 1):
            return Pattern(value(node.args[0]))
        raise ValueError(ast.dump(node))

    raw = SOURCE.read_bytes()
    for node in ast.parse(raw).body:
        if isinstance(node, ast.ClassDef):
            break
        if isinstance(node, ast.Assign) and len(node.targets) == 1:
            target = node.targets[0]
            if isinstance(target, ast.Name) and target.id.isupper():
                names[target.id] = value(node.value)
    return {
        'source_sha256': hashlib.sha256(raw).hexdigest(),
        'regex': {key: item.value for key, item in names.items() if isinstance(item, Pattern)},
        'words': {key: item for key, item in names.items()
                  if isinstance(item, list) and all(isinstance(word, str) for word in item)},
        'patterns': [item.value for item in names['PATTERNS']],
        'checked': [[item.value, check] for item, check in names['CHECKED']],
    }


def main():
    expected = json.dumps(policy(), indent=2, ensure_ascii=False) + '\n'
    if '--check' in sys.argv:
        if OUTPUT.read_text() != expected:
            raise SystemExit('Rust redaction policy is stale; run crates/controller/tools/redaction_policy.py')
    else:
        OUTPUT.write_text(expected)


if __name__ == '__main__':
    main()
