"""Submit through PrismaBuild. Test submissions inside the suite use only fakes."""
from pathlib import Path
import sys

import pytest

root = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(root / "harness/src"))
raise SystemExit(pytest.main([str(root / "workers/tests"), str(root / "harness/tests"), "-q"]))
