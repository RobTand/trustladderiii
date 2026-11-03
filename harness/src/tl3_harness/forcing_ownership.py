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

    source(module) returns its pinned AST and package status, or None. Dynamic
    bindings are a capability limit; the admitted runner still checks real objects.
    """
    def reject(module, category, reason):
        raise ForcingError(target=target, module=module, category=category, reason=reason)

    def bindings(body, name):
        found = []
        for node in body:
            if isinstance(node, (ast.ClassDef, ast.FunctionDef, ast.AsyncFunctionDef)) and node.name == name:
                found.append(node)
            elif isinstance(node, (ast.Import, ast.ImportFrom)):
                found.extend((node, alias) for alias in node.names
                             if (alias.asname or (alias.name.split('.')[0] if isinstance(node, ast.Import) else alias.name)) == name)
            elif isinstance(node, (ast.Assign, ast.AnnAssign)):
                targets = node.targets if isinstance(node, ast.Assign) else [node.target]
                if any(isinstance(item, ast.Name) and item.id == name for item in targets):
                    found.append(node)
        return found

    def resolve(parts, depth=0):
        if depth > 32:
            reject('.'.join(parts), 'unresolved', 'static_resolution_limit')
        for split in range(len(parts), 0, -1):
            module = '.'.join(parts[:split])
            try:
                parsed = source(module)
            except ForcingError as error:
                refusal = error.refusal
                raise ForcingError(target=target, module=refusal['resolved_module'],
                                   category=refusal['owner_category'], reason=refusal['reason']) from error
            if parsed is not None:
                tree, package = parsed
                body = tree.body
                break
        else:
            return '.'.join(parts), None, 'external_module'
        category = 'repository_module'
        for index, name in enumerate(parts[split:], split):
            found = bindings(body, name)
            if len(found) != 1:
                reject(module, 'unresolved', 'missing_or_dynamic_attribute')
            node = found[0]
            if isinstance(node, tuple):
                statement, alias = node
                if isinstance(statement, ast.Import):
                    imported = alias.name if alias.asname else alias.name.split('.')[0]
                else:
                    parent = module.split('.') if package else module.split('.')[:-1]
                    if statement.level:
                        parent = parent[:len(parent) - statement.level + 1]
                        imported = '.'.join([*parent, *filter(None, (statement.module,)), alias.name])
                    else:
                        imported = '.'.join(filter(None, (statement.module, alias.name)))
                return resolve([*imported.split('.'), *parts[index + 1:]], depth + 1)
            if isinstance(node, ast.ClassDef):
                if node.decorator_list:
                    reject(module, 'unresolved', 'dynamic_class_owner')
                body, category = node.body, 'repository_class'
            elif isinstance(node, (ast.Assign, ast.AnnAssign)):
                value = node.value
                suffix = []
                while isinstance(value, ast.Attribute):
                    suffix.insert(0, value.attr)
                    value = value.value
                if isinstance(value, ast.Name):
                    return resolve([*module.split('.'), value.id, *suffix, *parts[index + 1:]], depth + 1)
                reject(module, 'nonclass_owner', 'unsupported_owner')
            else:
                reject(module, 'nonclass_owner', 'unsupported_owner')
        return module, body, category

    parts = target.split('.')
    module, body, category = resolve(parts[:-1])
    if category == 'external_module':
        reject(module, category, 'outside_served_repository')
    if not bindings(body, parts[-1]):
        reject(module, category, 'missing_or_dynamic_attribute')
