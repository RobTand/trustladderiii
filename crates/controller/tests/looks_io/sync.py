"""Refresh compiled shell copies; source assertions in looks_io.rs reject drift."""
from pathlib import Path
import subprocess
import hashlib

ROOT = Path(__file__).resolve().parents[4]
HERE = Path(__file__).resolve().parent
BASE = 'cdacce8f99c28ca8dba8f5f76831b77b4dfcd836'
# The historical source and TD-adapted reference are retained separately.
# Only quota_allowance remains at #828's audited scripted extraction; real
# model reservations remain covered by quota_hold's controller interfaces.
QUOTA_BASE = '8b49b5cdff79a1a56290e3e6aa0572c4518c1e7c'
REFERENCE_SHA256 = 'd8ae723fb4b371fa0723c78e9ee8618ea3dc27c5cadfeed048fd1cf819197e90'
FROZEN_METHODS = {'quota_allowance'}
METHODS = ['look_state', 'look_alive', 'finish_stall', 'rearm', 'look_failed',
           'look_observations', 'probe_passed', 'save_allowance', 'allowance_row',
           'progress_allowance', 'quota_allowance', 'expire_model_quotas']
FUNCTIONS = ['incident_needs_rob', 'incident_id', 'incident_note', 'failure_class',
             'incident_summary']


def function(source, name, method):
    start = source.index('fn ' + name + '(')
    start = source.rfind('\n', 0, start) + 1
    end = source.index('\n    }\n' if method else '\n}\n', start)
    return source[start:end + (7 if method else 3)]


def main():
    base = subprocess.check_output(['git', 'show', BASE + ':crates/controller/src/looks.rs'], cwd=ROOT).decode()
    assert (HERE / 'base-looks.rs.txt').read_text() == base
    # The reference's expanded decisions were migrated in the atomic TD cut.
    # A refresh must not silently replace them with current implementation.
    assert hashlib.sha256((HERE / 'reference.rs').read_bytes()).hexdigest() == REFERENCE_SHA256
    quota_base = subprocess.check_output(['git', 'show', QUOTA_BASE + ':crates/controller/src/looks.rs'], cwd=ROOT).decode()
    for name, source in [('current', (ROOT / 'crates/controller/src/looks.rs').read_text())]:
        text = 'use super::*;\ncontroller!();\nimpl Controller {\n'
        text += ''.join(function(quota_base if name == 'current' and n in FROZEN_METHODS else source, n, True) for n in METHODS) + '}\n'
        text += ''.join(function(source, n, False) for n in FUNCTIONS)
        if name == 'current':
            text += function(source, 'incident_view', False)
            text += function(source, 'stall_upstream', False)
            text += function(source, 'look_kind', False)
            text += function(source, 'wait_status', False)
        (HERE / (name + '.rs')).write_text(text)
    for method, path in [('look_state', 'look-state-current.rs'),
                         ('look_observations', 'look-observations-current.rs')]:
        (HERE / path).write_text(function(source, method, True))


if __name__ == '__main__':
    main()
