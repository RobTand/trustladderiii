"""Forcing ownership diagnostics and conservative source-only admission."""
import ast
from pathlib import Path
import sys
import types


class ForcingError(Exception):
    """An unsupported forcing, never an assertion verdict."""

    def __init__(self, message=None, *, target=None, module='unknown', category='unknown', reason='unsupported_owner'):
        self.refusal = None
        if target is not None:
            self.refusal = {'phase': 'forcing_validation', 'target': target,
                            'resolved_module': module, 'owner_category': category, 'reason': reason}
            message = f'Forcing validation refused {target}: resolved module {module}, owner {category}, {reason}.'
        super().__init__(message)


def repository_owner(target, owner, root):
    """The runner's object-level guard. An alias confers no repository ownership."""
    module = owner if isinstance(owner, types.ModuleType) else sys.modules.get(getattr(owner, '__module__', ''))
    source = getattr(module, '__file__', None) if isinstance(owner, (types.ModuleType, type)) else None
    resolved = getattr(module, '__name__', 'unknown')
    if not source or not Path(source).resolve().is_relative_to(root):
        raise ForcingError(target=target, module=resolved, category=(
            'external_module' if isinstance(owner, types.ModuleType) else 'external_or_nonclass_owner'),
            reason='outside_served_repository')
    if not hasattr(owner, target.split('.')[-1]):
        raise ForcingError(target=target, module=resolved, category='repository', reason='missing_attribute')


def preflight_owner(target, source):
    """Resolve only explicit source bindings, without importing served code.

    source(module) returns its pinned AST and package status, or None. Only a
    definite external import is refused here. The admitted runner checks real
    objects, including inherited attributes and conditional or dynamic bindings.
    """
    def reject(module, category, reason):
        raise ForcingError(target=target, module=module, category=category, reason=reason)

    def bindings(body, name):
        found = []
        for node in body:
            # Source inspection cannot establish the resulting namespace when
            # executable statements may replace bindings. Leave it to the runner.
            if isinstance(node, (ast.If, ast.For, ast.AsyncFor, ast.While, ast.Try, ast.TryStar,
                                 ast.With, ast.AsyncWith, ast.Match, ast.Delete, ast.AugAssign)):
                return []
            if isinstance(node, ast.Expr) and not isinstance(node.value, ast.Constant):
                return []
            if isinstance(node, ast.ImportFrom) and any(alias.name == '*' for alias in node.names):
                return []
            if isinstance(node, (ast.ClassDef, ast.FunctionDef, ast.AsyncFunctionDef)) and node.name == name:
                found.append(node)
            elif isinstance(node, (ast.Import, ast.ImportFrom)):
                found.extend((node, alias) for alias in node.names
                             if (alias.asname or (alias.name.split('.')[0] if isinstance(node, ast.Import) else alias.name)) == name)
            elif isinstance(node, (ast.Assign, ast.AnnAssign)):
                targets = node.targets if isinstance(node, ast.Assign) else [node.target]
                if (any(not isinstance(item, ast.Name) for item in targets)
                        or node.value is not None and any(isinstance(item, (ast.Call, ast.NamedExpr))
                                                          for item in ast.walk(node.value))):
                    return []
                if any(isinstance(item, ast.Name) and item.id == name for item in targets):
                    found.append(node)
        return found

    def resolve(parts, depth=0, imported=False):
        if depth > 32:
            return None
        for split in range(len(parts), 0, -1):
            module = '.'.join(parts[:split])
            try:
                parsed = source(module)
            except ForcingError:
                return None
            if parsed is not None:
                tree, package = parsed
                body = tree.body
                break
        else:
            return '.'.join(parts) if imported else None
        for index, name in enumerate(parts[split:], split):
            found = bindings(body, name)
            if len(found) != 1:
                return None
            node = found[0]
            if isinstance(node, tuple):
                statement, alias = node
                if isinstance(statement, ast.Import):
                    imported_path = alias.name if alias.asname else alias.name.split('.')[0]
                else:
                    parent = module.split('.') if package else module.split('.')[:-1]
                    if statement.level:
                        parent = parent[:len(parent) - statement.level + 1]
                        imported_path = '.'.join([*parent, *filter(None, (statement.module,)), alias.name])
                    else:
                        imported_path = '.'.join(filter(None, (statement.module, alias.name)))
                return resolve([*imported_path.split('.'), *parts[index + 1:]], depth + 1, imported=True)
            if isinstance(node, ast.ClassDef):
                if node.decorator_list:
                    return None
                body = node.body
            elif isinstance(node, (ast.Assign, ast.AnnAssign)):
                value = node.value
                suffix = []
                while isinstance(value, ast.Attribute):
                    suffix.insert(0, value.attr)
                    value = value.value
                if isinstance(value, ast.Name):
                    return resolve([*module.split('.'), value.id, *suffix, *parts[index + 1:]], depth + 1)
                return None
            else:
                return None
        return None

    parts = target.split('.')
    module = resolve(parts[:-1])
    if module is not None:
        reject(module, 'external_module', 'outside_served_repository')
