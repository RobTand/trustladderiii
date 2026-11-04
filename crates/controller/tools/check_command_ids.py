"""#771 source policy: command IDs come from the kernel's checked allocator."""
from pathlib import Path
import re
import sys

source = Path(__file__).resolve().parents[1] / 'src'
violations = []
for path in sorted(source.rglob('*.rs')):
    for match in re.finditer(r'\blast_cmd\s*(?:\+|\+=)', path.read_text()):
        line = path.read_text().count('\n', 0, match.start()) + 1
        violations.append(f'{path.relative_to(source)}:{line}: unchecked command ID arithmetic')
if violations:
    sys.exit('\n'.join(violations))
print('command ID policy: no unchecked last_cmd increment in controller sources')
