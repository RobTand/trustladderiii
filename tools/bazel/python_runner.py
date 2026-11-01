"""Run one discovered module and write a parity receipt only after success."""
import json
import os
from pathlib import Path
import sys
import unittest

# Import siblings by path: a crate's own tools/ directory can shadow ours.
sys.path.insert(0, str(Path(__file__).parent))
from runtime import prepare  # noqa: E402


def main():
    prepare()
    relative = sys.argv[1]
    module = Path(relative).resolve()
    # The deployment tests drive the helper that Bazel built for this test.
    os.environ['TL3_DEPLOY_HELPER'] = str(Path('crates/controller/example_tl3-deploy').resolve())
    sys.path.insert(0, str(module.parent))
    if relative.startswith(('workers/', 'harness/')):
        # Light checks run these projects' pytest from the project directory.
        import pytest
        os.chdir(relative.split('/')[0])
        status = pytest.main([str(module), '-v'])
    else:
        suite = unittest.defaultTestLoader.loadTestsFromName(module.stem)
        status = 0 if unittest.TextTestRunner(verbosity=2).run(suite).wasSuccessful() else 1
    if status:
        return int(status)
    Path(os.environ['TEST_UNDECLARED_OUTPUTS_DIR'], 'inventory.json').write_text(
        json.dumps({'python': [relative]}))
    return 0


if __name__ == '__main__':
    sys.exit(main())
