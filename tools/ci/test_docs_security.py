"""Keep credential-leaking BuildBuddy commands out of docs and agent briefs."""
from pathlib import Path
import subprocess
import unittest

from child_env import child_env


ROOT = Path(__file__).resolve().parents[2]


def is_instruction(path):
    return (path.parts[0] == 'docs'
            or path.as_posix() in ('AGENTS.md', 'CLAUDE.md')
            or (path.parts[0] == '.agents' and path.suffix == '.md'))


def instruction_files(root):
    if (root / '.git').exists():
        tracked = subprocess.run(
            ['git', 'ls-files', '-z'], cwd=root, env=child_env(),
            check=True, capture_output=True, text=True,
        ).stdout.split('\0')
        return sorted(Path(name) for name in tracked if name and is_instruction(Path(name)))
    # Bazel runfiles contain only the declared, tracked instruction inputs.
    candidates = [*root.glob('docs/**/*'), *root.glob('.agents/**/*.md'),
                  root / 'AGENTS.md', root / 'CLAUDE.md']
    return sorted(path.relative_to(root) for path in candidates if path.is_file())


class DocsSecurityTests(unittest.TestCase):
    def test_docs_and_agent_briefs_forbid_remote_cli(self):
        paths = instruction_files(ROOT)
        self.assertIn(Path('docs/bazel.md'), paths, 'Bazel must declare the docs inputs')
        violations = []
        for path in paths:
            for number, line in enumerate((ROOT / path).read_bytes().splitlines(), 1):
                if b'bb remote' in line:
                    violations.append(f'{path}:{number}')
        self.assertEqual(violations, [],
                         'Use tools/ci/remote_gate.py, not the credential-leaking CLI: '
                         + ', '.join(violations))

    def test_scope_includes_all_docs_and_only_the_named_agent_briefs(self):
        for name in ('docs/bazel.md', 'docs/nested/evidence.json', 'docs/notes.txt',
                     'AGENTS.md', 'CLAUDE.md', '.agents/brief.md', '.agents/nested/brief.md'):
            with self.subTest(path=name):
                self.assertTrue(is_instruction(Path(name)))
        for name in ('README.md', 'tools/ci/test_docs_security.py',
                     'docs-old/notes.md', 'nested/AGENTS.md', '.agents/config.json'):
            with self.subTest(path=name):
                self.assertFalse(is_instruction(Path(name)))


if __name__ == '__main__':
    unittest.main()
