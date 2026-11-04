"""Keep the committed Rust parity oracle tied to the actual Python tokenizer."""
import json
from pathlib import Path
import runpy

import pytest


ROOT = Path(__file__).resolve().parents[2]
GENERATOR = ROOT / "crates/config/tools/tokenizer_oracle.py"
ORACLE = ROOT / "crates/config/tests/tokenizer/oracle.json"


def test_committed_tokenizer_oracle_matches_python_and_reports_speed(monkeypatch, capsys):
    generator = runpy.run_path(str(GENERATOR))
    before = ORACLE.read_bytes(), ORACLE.stat().st_mtime_ns
    monkeypatch.setattr("sys.argv", ["tokenizer_oracle.py", "--check", "--benchmark"])
    with capsys.disabled():
        status = generator["main"]()
    assert status == 0, "--check rejected the committed oracle"
    assert (ORACLE.read_bytes(), ORACLE.stat().st_mtime_ns) == before


def test_source_inputs_cover_both_byte_boundaries_in_several_languages():
    generator = runpy.run_path(str(GENERATOR))
    cases = {case["name"]: case for case in generator["corpus"]()}
    for language in ["rust", "python", "typescript", "go"]:
        for size in [65_535, 65_536]:
            name = f"source-boundary-{language}-{size}"
            assert name in cases, f"missing representative source input: {name}"
            text = generator["expanded"](cases[name])
            assert len(text.encode()) == size
            assert text.count("\n") > 100
            assert "\n    " in text or "\n\t" in text


def test_check_mode_accepts_clean_and_rejects_hand_edited_oracle_without_writes(tmp_path, monkeypatch, capsys):
    generator = runpy.run_path(str(GENERATOR))
    path = tmp_path / "oracle.json"
    monkeypatch.setattr("sys.argv", ["tokenizer_oracle.py", "--output", str(path)])
    assert generator["main"]() == 0
    before = path.read_bytes(), path.stat().st_mtime_ns
    monkeypatch.setattr("sys.argv", ["tokenizer_oracle.py", "--check", "--output", str(path)])
    with pytest.raises(SystemExit) as clean:
        runpy.run_path(str(GENERATOR), run_name="__main__")
    assert clean.value.code == 0
    assert (path.read_bytes(), path.stat().st_mtime_ns) == before

    edited = json.loads(path.read_bytes())
    edited["cases"][0]["tokens"] += 1
    path.write_text(json.dumps(edited) + "\n")
    before = path.read_bytes(), path.stat().st_mtime_ns
    with pytest.raises(SystemExit) as drift:
        runpy.run_path(str(GENERATOR), run_name="__main__")
    assert drift.value.code == 1
    assert "oracle drift:" in capsys.readouterr().err
    assert (path.read_bytes(), path.stat().st_mtime_ns) == before

    missing = tmp_path / "missing.json"
    monkeypatch.setattr("sys.argv", ["tokenizer_oracle.py", "--check", "--output", str(missing)])
    assert generator["main"]() == 1
    assert not missing.exists()
