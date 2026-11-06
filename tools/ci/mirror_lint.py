"""Reject source copies and stale exceptions (PRD #878, M1)."""

import argparse
import ast
from collections import Counter
import hashlib
import io
import json
from pathlib import Path
import re
import subprocess
import tokenize


ROOT = Path(__file__).resolve().parents[2]
ALLOWLIST = Path('tools/ci/mirror-allowlist.json')
MANIFEST = Path('crates/contracts/generated/manifest.json')
DRIFT_TEST = Path('crates/contracts/tests/schema_drift.rs')
TEST_DIRS = {'test', 'tests', 'fixture', 'fixtures', 'testdata', '__tests__'}
DECLARATION = re.compile(
    r'\b(?:mirror\s+of\b|(?<!git )mirrors\s+(?!(?:and|or|are|the|a|an|in|for|to|from)\b)'
    r'[a-z_][\w.:/]*|read-only\s+mirror\b|MIRROR\s*:)',
    re.IGNORECASE,
)
RUST_TOKEN = re.compile(
    r'(?P<comment>//[^\n]*|/\*[\s\S]*?\*/)'
    r'|(?P<string>(?:b?r(?P<hash>\#*)"[\s\S]*?"(?P=hash))|b?"(?:\\.|[^"\\])*")'
    r"|'(?:\\.|[^'\\\n])'|[a-zA-Z_]\w*|[^\s]",
)


def is_test(path):
    return (bool(TEST_DIRS.intersection(path.parts))
            or path.stem in {'test', 'tests'}
            or path.stem.startswith(('test_', 'tests_')) or path.stem.endswith(('_test', '_tests')))


def is_production(path):
    return path.suffix in {'.rs', '.py'} and not is_test(path)


def rust_tokens(text):
    return [token for token in RUST_TOKEN.finditer(text) if token.lastgroup != 'comment']


def rust_test_file(tokens):
    values = [token.group() for token in tokens]
    depth = 0
    for index, value in enumerate(values):
        if depth == 0 and values[index:index + 8] == ['#', '!', '[', 'cfg', '(', 'test', ')', ']']:
            return True
        if value == '{':
            depth += 1
        elif value == '}':
            depth -= 1
    return False


def rust_test_spans(tokens):
    """Locate cfg(test) items and #[test] functions without counting string braces."""
    if rust_test_file(tokens):
        return [(0, tokens[-1].end())]
    spans = []
    values = [token.group() for token in tokens]
    for start in range(len(tokens)):
        if values[start:start + 2] != ['#', '[']:
            continue
        try:
            end = values.index(']', start + 2)
        except ValueError:
            continue
        attribute = ''.join(values[start + 2:end])
        if attribute != 'test' and not (
            attribute.startswith(('cfg(test)', 'cfg(all(test,', 'cfg(any(test,'))
        ):
            continue
        depth = 0
        for index in range(end + 1, len(tokens)):
            if values[index] == '{':
                depth += 1
            elif values[index] == '}':
                depth -= 1
                if depth == 0:
                    spans.append((tokens[start].start(), tokens[index].end()))
                    break
            elif values[index] == ';' and depth == 0:
                spans.append((tokens[start].start(), tokens[index].end()))
                break
    merged = []
    for start, end in sorted(spans):
        if merged and start <= merged[-1][1]:
            merged[-1] = (merged[-1][0], max(end, merged[-1][1]))
        else:
            merged.append((start, end))
    return merged


def rust_includes(tokens):
    for index, token in enumerate(tokens):
        if token.group() not in {'include_str', 'include_bytes'}:
            continue
        tail = tokens[index + 1:index + 5]
        if len(tail) == 4 and [t.group() for t in tail[:2]] == ['!', '('] \
                and tail[2].lastgroup == 'string' and tail[3].group() in {')', ','}:
            literal = tail[2].group()
            if literal.startswith('r'):
                value = literal[literal.index('"') + 1:literal.rindex('"')]
            else:
                value = ast.literal_eval(literal)
            yield token.start(), value


def python_tree(text):
    try:
        return ast.parse(text)
    except SyntaxError:
        # Fixtures can deliberately contain invalid Python. Comments still count.
        return ast.Module(body=[], type_ignores=[])


def python_comments(text, tree):
    comments = []
    try:
        for token in tokenize.generate_tokens(io.StringIO(text).readline):
            if token.type == tokenize.COMMENT:
                comments.append(token.string)
    except (tokenize.TokenError, IndentationError):
        pass
    for node in ast.walk(tree):
        if isinstance(node, (ast.Module, ast.ClassDef, ast.FunctionDef, ast.AsyncFunctionDef)):
            doc = ast.get_docstring(node)
            if doc:
                comments.append(doc)
    return comments


def literal_path(node, bindings, file):
    """Evaluate only literal pathlib operations; never execute repository code."""
    if isinstance(node, ast.Constant) and isinstance(node.value, str):
        return Path(node.value)
    if isinstance(node, ast.Name):
        return file if node.id == '__file__' else bindings.get(node.id)
    if isinstance(node, ast.BinOp) and isinstance(node.op, ast.Div):
        left = literal_path(node.left, bindings, file)
        right = literal_path(node.right, bindings, file)
        if left is not None and right is not None:
            return left / right
    if isinstance(node, ast.Attribute) and node.attr == 'parent':
        value = literal_path(node.value, bindings, file)
        return value.parent if value is not None else None
    if isinstance(node, ast.Subscript) and isinstance(node.value, ast.Attribute) \
            and node.value.attr == 'parents' and isinstance(node.slice, ast.Constant) \
            and isinstance(node.slice.value, int):
        value = literal_path(node.value.value, bindings, file)
        if value is not None and 0 <= node.slice.value < len(value.parents):
            return value.parents[node.slice.value]
    if isinstance(node, ast.Call):
        name = node.func.id if isinstance(node.func, ast.Name) else (
            node.func.attr if isinstance(node.func, ast.Attribute) else '')
        if name in {'Path', 'PurePath'} and node.args:
            parts = [literal_path(arg, bindings, file) for arg in node.args]
            return Path(*parts) if all(part is not None for part in parts) else None
        if isinstance(node.func, ast.Attribute):
            value = literal_path(node.func.value, bindings, file)
            if value is not None:
                if name in {'resolve', 'absolute'} and not node.args:
                    return value
                if name == 'joinpath':
                    parts = [literal_path(arg, bindings, file) for arg in node.args]
                    if all(part is not None for part in parts):
                        return value.joinpath(*parts)
    return None


def python_accesses(tree, file):
    """Collect literal reads and writes; mutation helpers do not freeze source text."""
    bindings = {}
    reads, writes = [], []
    for node in ast.walk(tree):
        if isinstance(node, (ast.Assign, ast.AnnAssign)):
            value = literal_path(node.value, bindings, file)
            targets = node.targets if isinstance(node, ast.Assign) else [node.target]
            for target in targets:
                if isinstance(target, ast.Name):
                    bindings[target.id] = value
        if not isinstance(node, ast.Call):
            continue
        mode = 'r'
        if isinstance(node.func, ast.Attribute) and node.func.attr in {
            'read_text', 'read_bytes', 'write_text', 'write_bytes', 'open',
        }:
            value = literal_path(node.func.value, bindings, file)
            if node.func.attr.startswith('write_'):
                mode = 'w'
            elif node.func.attr == 'open' and node.args:
                mode = node.args[0].value if isinstance(node.args[0], ast.Constant) else None
        elif isinstance(node.func, ast.Name) and node.func.id == 'open' and node.args:
            value = literal_path(node.args[0], bindings, file)
            if len(node.args) > 1:
                mode = node.args[1].value if isinstance(node.args[1], ast.Constant) else None
        else:
            continue
        for keyword in node.keywords:
            if keyword.arg == 'mode':
                mode = keyword.value.value if isinstance(keyword.value, ast.Constant) else None
        if value is not None:
            if isinstance(mode, str) and any(flag in mode for flag in 'wax+'):
                writes.append(value)
            if mode is None or (isinstance(mode, str) and ('r' in mode or '+' in mode)):
                reads.append(value)
    return reads, writes


def source_paths(value, file, root, production):
    paths = set()
    for candidate in (file.parent / value, root / value):
        resolved = candidate.resolve()
        if resolved.is_relative_to(root) and resolved.relative_to(root) in production:
            paths.add(resolved.relative_to(root))
    return paths


def generated_paths(root):
    manifest = root / MANIFEST
    if manifest.exists():
        data = json.loads(manifest.read_text())
        # Accept a path list or file records, with an optional top-level files key.
        records = data.get('files', data.get('outputs')) if isinstance(data, dict) else data
        if not isinstance(records, list):
            raise ValueError(f'{MANIFEST}: expected files/outputs list')
        paths = set()
        for record in records:
            path = record['path'] if isinstance(record, dict) else record
            if not isinstance(path, str) or Path(path).is_absolute() or '..' in Path(path).parts:
                raise ValueError(f'{MANIFEST}: invalid output path')
            paths.add(Path(path))
        return paths
    drift = root / DRIFT_TEST
    if not drift.exists():
        return set()
    return {(drift.parent / value).resolve().relative_to(root)
            for _, value in rust_includes(rust_tokens(drift.read_text()))}


def shingles(text, suffix):
    """Hash windows of twelve meaningful lines; retain lines to reject hash collisions."""
    lines = []
    importing = False
    import_lines = set()
    if suffix == '.py':
        tree = python_tree(text)
        for node in ast.walk(tree):
            if isinstance(node, (ast.Import, ast.ImportFrom)):
                import_lines.update(range(node.lineno, node.end_lineno + 1))
            if isinstance(node, (ast.Module, ast.ClassDef, ast.FunctionDef, ast.AsyncFunctionDef)):
                doc = ast.get_docstring(node)
                if doc and re.search(r'SPDX-License-Identifier|copyright|permission is hereby granted', doc, re.I):
                    expression = node.body[0]
                    import_lines.update(range(expression.lineno, expression.end_lineno + 1))
        try:
            for token in tokenize.generate_tokens(io.StringIO(text).readline):
                if token.type == tokenize.COMMENT and not token.line[:token.start[1]].strip():
                    import_lines.add(token.start[0])
        except (tokenize.TokenError, IndentationError):
            pass
    else:
        # Mask Rust comments without turning comment markers in literals into comments.
        parts, cursor = [], 0
        for token in RUST_TOKEN.finditer(text):
            if token.lastgroup == 'comment':
                parts.extend((text[cursor:token.start()], re.sub(r'[^\n]', ' ', token.group())))
                cursor = token.end()
        text = ''.join([*parts, text[cursor:]])
    for number, raw in enumerate(text.splitlines(), 1):
        if number in import_lines:
            continue
        line = raw.strip()
        if suffix == '.rs' and re.match(r'(?:pub(?:\([^)]*\))?\s+)?use\b', line):
            importing = ';' not in line
            continue
        if importing:
            importing = ';' not in line
            continue
        if re.match(r'(?:from\s+\S+\s+import\b|import\b|extern\s+crate\b)', line):
            continue
        if len(line) < 12 or not re.search(r'[\w]', line):
            continue
        lines.append(line)
    for index in range(len(lines) - 11):
        window = tuple(lines[index:index + 12])
        yield hashlib.sha256('\n'.join(window).encode()).digest(), window


def scan(root, files):
    """Return (repository path, detector) pairs for the supplied tracked files."""
    root = root.resolve()
    files = sorted(set(map(Path, files)))
    texts = {}
    for path in files:
        absolute = root / path
        if absolute.is_file() and not absolute.is_symlink():
            try:
                texts[path] = absolute.read_text()
            except UnicodeError:
                continue
    tokens_by_path = {path: rust_tokens(text) for path, text in texts.items() if path.suffix == '.rs'}
    test_files = {path for path in texts if is_test(path)}
    test_files.update(path for path, tokens in tokens_by_path.items() if rust_test_file(tokens))
    production = {path for path in texts if is_production(path) and path not in test_files}
    generated = generated_paths(root)
    offenders = set()
    production_windows = {}
    test_windows = {}
    for path, text in texts.items():
        absolute = root / path
        name = path.as_posix()
        tokens = tokens_by_path.get(path, [])
        spans = rust_test_spans(tokens) if tokens else []
        tree = python_tree(text) if path.suffix == '.py' else None
        if path.suffix == '.rs':
            comments = [m.group() for m in RUST_TOKEN.finditer(text) if m.lastgroup == 'comment']
            reads = (value for offset, value in rust_includes(tokens)
                     if path in test_files or any(start <= offset < end for start, end in spans))
            writes = []
        elif path.suffix == '.py':
            comments = python_comments(text, tree)
            reads, writes = python_accesses(tree, absolute) if path in test_files else ([], [])
        else:
            comments, reads, writes = [], [], []
        if any(DECLARATION.search(comment) for comment in comments):
            offenders.add((name, 'self-declared'))
        read_paths = set().union(*(source_paths(value, absolute, root, production) for value in reads))
        write_paths = set().union(*(source_paths(value, absolute, root, production) for value in writes))
        if read_paths - write_paths:
            offenders.add((name, 'source-include'))
        if path.name.endswith('.schema.json') or (path.suffix == '.json' and 'schemas' in path.parts):
            if path not in generated and not re.search(r'\b(?:auto-?generated|generated (?:by|from))\b', text, re.I):
                offenders.add((name, 'ungenerated-schema'))
        if path in production:
            for start, end in reversed(spans):
                text = text[:start] + '\n' + text[end:]
            for digest, window in shingles(text, path.suffix):
                production_windows.setdefault(digest, set()).add(window)
        elif path in test_files:
            test_windows[name] = list(shingles(text, path.suffix))
    for name, windows in test_windows.items():
        if any(window in production_windows.get(digest, ()) for digest, window in windows):
            offenders.add((name, 'frozen-copy'))
    return offenders


def ratchet(offenders, entries):
    """Return errors for malformed entries, new offenders and stale exceptions."""
    allowed = set()
    errors = []
    for entry in entries:
        if not isinstance(entry, dict) or set(entry) != {'path', 'detector', 'owner'} \
                or not all(isinstance(value, str) and value for value in entry.values()):
            errors.append('invalid allowlist entry: expected path, detector, owner strings')
            continue
        key = (entry['path'], entry['detector'])
        if entry['owner'] == 'unassigned' or not re.search(r'#\d+\b', entry['owner']):
            errors.append(f'{entry["path"]}: owner must name an issue with #<number>')
        if key in allowed:
            errors.append(f'duplicate allowlist entry: {key}')
        allowed.add(key)
    errors.extend(f'new offender: {path} [{detector}]' for path, detector in sorted(offenders - allowed))
    errors.extend(f'stale allowlist entry: {path} [{detector}]' for path, detector in sorted(allowed - offenders))
    return errors


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, default=ROOT)
    parser.add_argument('--inventory', action='store_true', help='emit unassigned entries without enforcing exceptions')
    args = parser.parse_args(argv)
    files = subprocess.check_output(['git', 'ls-files', '-z'], cwd=args.root).decode().split('\0')
    try:
        offenders = scan(args.root, [Path(name) for name in files if name])
        if args.inventory:
            print(json.dumps([{'path': path, 'detector': detector, 'owner': 'unassigned'}
                              for path, detector in sorted(offenders)], indent=2))
            return 0
        allowlist = args.root / ALLOWLIST
        entries = json.loads(allowlist.read_text()) if allowlist.exists() else []
        if not isinstance(entries, list):
            raise ValueError(f'{ALLOWLIST}: expected a list')
        errors = ratchet(offenders, entries)
    except (ValueError, OSError, KeyError) as error:
        print(f'mirror-lint: {error}')
        return 1
    for error in errors:
        print(error)
    print(f'mirror-lint: {len(offenders)} offenders; {len(errors)} errors; '
          f'detectors={dict(sorted(Counter(detector for _, detector in offenders).items()))}')
    return int(bool(errors))


if __name__ == '__main__':
    raise SystemExit(main())
