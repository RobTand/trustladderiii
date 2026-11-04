"""Keep the committed Rust parity oracle tied to the actual Python tokenizer."""
from pathlib import Path
import runpy


def test_committed_tokenizer_oracle_matches_python_and_reports_speed(tmp_path, monkeypatch):
    root = Path(__file__).resolve().parents[2]
    generator = runpy.run_path(str(root / "crates/config/tools/tokenizer_oracle.py"))
    output = tmp_path / "oracle.json"
    monkeypatch.setattr("sys.argv", ["tokenizer_oracle.py", "--output", str(output), "--benchmark"])
    generator["main"]()
    assert output.read_bytes() == (root / "crates/config/tests/tokenizer/oracle.json").read_bytes()
