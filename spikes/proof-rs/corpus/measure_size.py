"""Source inventory for the spike. Run on BuildBuddy or the admitted worker."""
import ast
import json
from pathlib import Path

source = Path('harness/src/tl3_harness/proof.py').read_text()
tree = ast.parse(source)
functions = {node.name: {'start': node.lineno, 'end': node.end_lineno, 'physical': node.end_lineno - node.lineno + 1}
             for node in tree.body if isinstance(node, (ast.FunctionDef, ast.ClassDef))}
prototype = Path('spikes/proof-rs/src/main.rs').read_text()

def size(text):
    lines = text.splitlines()
    return {'physical': len(lines), 'nonblank_noncomment': sum(bool(line.strip()) and not line.lstrip().startswith(('#', '//')) for line in lines)}

# A conditional size estimate, not a proposed change to the current validator.
# Drop call restrictions and the omitted-context checks, but retain namespace
# writes, wildcard imports, graft binding conflicts, and source/tree checks.
removed = ['validate_support_calls', 'validate_partial_definition_uses', 'validate_partial_literal']
spans = {(functions[name]['start'], functions[name]['end']) for name in removed}
remaining = '\n'.join(line for index, line in enumerate(source.splitlines(), 1)
                      if not any(start <= index <= end for start, end in spans))
rust_lines = prototype.splitlines()
rust_functions = {}
for index, line in enumerate(rust_lines):
    if line.startswith('fn '):
        name = line[3:].split('(')[0].split('<')[0]
        end = next(i for i in range(index + 1, len(rust_lines)) if rust_lines[i] == '}')
        rust_functions[name] = {'start': index + 1, 'end': end + 1, 'physical': end - index + 1}
rust_removed = ['support_calls', 'partial']
rust_remaining = '\n'.join(line for index, line in enumerate(rust_lines, 1)
                           if not any(rust_functions[name]['start'] <= index <= rust_functions[name]['end'] for name in rust_removed))
# Conservative deletion inventory keeps literal shape/assignment validation and
# namespace checks. It is still a size estimate, without replacement code.
python_conditional_spans = [(functions[name]['start'], functions[name]['end'])
                            for name in ('validate_support_calls', 'validate_partial_definition_uses')]
python_conditional_spans += [(141, 157), (167, 187)]
python_conservative = '\n'.join(line for index, line in enumerate(source.splitlines(), 1)
                               if not any(start <= index <= end for start, end in python_conditional_spans))
partial_start = rust_functions['partial']['start'] - 1
definition_start = next(i for i in range(partial_start, len(rust_lines)) if 'let is_def = name(s).is_some();' in rust_lines[i])
definition_end = next(i for i in range(definition_start, len(rust_lines)) if 'let value = match s {' in rust_lines[i])
literal_start = next(i for i in range(definition_end, len(rust_lines)) if 'let binding = if let Stmt::Assign(n) = s {' in rust_lines[i])
literal_end = next(i for i in range(literal_start, len(rust_lines)) if 'namespace(&parse(module_source, module)?' in rust_lines[i])
rust_conditional_spans = [(rust_functions['support_calls']['start'], rust_functions['support_calls']['end']),
                          (definition_start + 1, definition_end), (literal_start + 1, literal_end)]
rust_conservative = '\n'.join(line for index, line in enumerate(rust_lines, 1)
                             if not any(start <= index <= end for start, end in rust_conditional_spans))
print(json.dumps({'rust_functions': rust_functions, 'conditional_rust_removed_functions': rust_removed,
                  'conservative_python_spans': python_conditional_spans,
                  'conservative_rust_spans': rust_conditional_spans,
                  'conservative_python_residue': size(python_conservative),
                  'conservative_rust_residue': size(rust_conservative),
                  'conditional_rust_residue': size(rust_remaining), 'python': size(source), 'rust': size(prototype), 'functions': functions,
                  'conditional_removed_functions': removed, 'conditional_python_residue': size(remaining),
                  'caveat': 'Residue counts exclude replacement checks and stale call-site cleanup. This is a source inventory, not a working reduced port.'}, indent=2))
