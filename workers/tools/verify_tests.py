"""Submit through PrismaBuild. Test submissions inside the suite use only fakes."""
from pathlib import Path

import pytest

root = Path(__file__).resolve().parents[2]
raise SystemExit(pytest.main([str(root / "workers/tests"), str(root / "harness/tests"),
                             "--ignore=" + str(root / "harness/tests/fixtures"), "-q"]))
