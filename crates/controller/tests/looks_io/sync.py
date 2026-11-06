"""Refresh compiled shell copies; source assertions in looks_io.rs reject drift."""
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[4]
HERE = Path(__file__).resolve().parent
BASE = 'cdacce8f99c28ca8dba8f5f76831b77b4dfcd836'
# #828 intentionally changes these Q53 shells after the extraction. Keep the
# original extraction comparison, with real quota paths covered by quota_hold.
QUOTA_BASE = '8b49b5cdff79a1a56290e3e6aa0572c4518c1e7c'
FROZEN_METHODS = {'look_state', 'look_observations', 'quota_allowance'}
METHODS = ['look_state', 'look_alive', 'finish_stall', 'rearm', 'look_failed',
           'look_observations', 'probe_passed', 'save_allowance', 'allowance_row', 'record_progress',
           'progress_allowance', 'quota_allowance', 'expire_model_quotas']
FUNCTIONS = ['incident_needs_rob', 'incident_id', 'incident_note', 'failure_class',
             'incident_summary', 'queue_command_detail']


def function(source, name, method):
    start = source.index('fn ' + name + '(')
    start = source.rfind('\n', 0, start) + 1
    end = source.index('\n    }\n' if method else '\n}\n', start)
    return source[start:end + (7 if method else 3)]


def main():
    base = subprocess.check_output(['git', 'show', BASE + ':crates/controller/src/looks.rs'], cwd=ROOT).decode()
    (HERE / 'base-looks.rs.txt').write_text(base)
    quota_base = subprocess.check_output(['git', 'show', QUOTA_BASE + ':crates/controller/src/looks.rs'], cwd=ROOT).decode()
    for name, source in [('reference', base), ('current', (ROOT / 'crates/controller/src/looks.rs').read_text())]:
        text = 'use super::*;\ncontroller!();\nimpl Controller {\n'
        text += ''.join(function(quota_base if name == 'current' and n in FROZEN_METHODS else source, n, True) for n in METHODS) + '}\n'
        text += ''.join(function(source, n, False) for n in FUNCTIONS)
        if name == 'current':
            text += function(source, 'incident_view', False)
            text += function(source, 'stall_upstream', False)
            text += function(source, 'look_kind', False)
            text += function(source, 'wait_status', False)
        (HERE / (name + '.rs')).write_text(text)
    types = base[base.index('#[derive(Clone, Debug, Serialize, Deserialize)]'):base.index('pub(crate) fn resolve_effect_incidents')]
    # The signature's serde boundary remains real; AttachedIncident is unused here.
    sig = base[base.index('#[derive(Clone, Debug, Serialize, Deserialize)]\n#[serde(try_from'):base.index('pub(crate) enum IncidentCause')]
    cause = base[base.index('pub(crate) enum IncidentCause'):base.index('pub(crate) fn incident_needs_rob')]
    (HERE / 'types.rs').write_text(types + sig + cause)


if __name__ == '__main__':
    main()
