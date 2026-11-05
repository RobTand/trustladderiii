"""Refresh the current compiled shell, never the frozen reference or base pin.

This is fixture generation, not a second implementation of any decision.
"""
from pathlib import Path

ROOT = Path(__file__).resolve().parents[4]
HERE = Path(__file__).resolve().parent
SOURCE = ROOT / 'crates/controller/src/adapters/workers/review.rs'
text = SOURCE.read_text()
start = text.index('pub(super) fn inputs(')
end = text.index('pub(crate) fn criteria(')
(HERE / 'current.rs').write_text('use super::*;\n' + text[start:end])
