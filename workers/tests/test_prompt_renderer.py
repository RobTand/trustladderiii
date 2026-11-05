"""Selected-release transport uses recorded Rust replies, without a Rust binary."""
from pathlib import Path
import sys

import pytest

import prompt_render_stub
from tl3_workers import prompt_render


@pytest.mark.parametrize('row', prompt_render_stub.cases())
def test_selected_release_transport(row, tmp_path, monkeypatch):
    binary = tmp_path / 'selected-release-renderer'
    binary.write_text(f'#!{sys.executable}\nimport runpy\n'
                      f'runpy.run_path({str(Path(prompt_render_stub.__file__).resolve())!r}, run_name="__main__")\n')
    binary.chmod(0o755)
    monkeypatch.setenv('TL3_RELEASE_BINARY', str(binary))
    assert prompt_render.render(row['kind'], row['document']) == row['response']
