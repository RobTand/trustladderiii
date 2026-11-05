"""Refresh compiled shell copies; source assertions in looks_io.rs reject drift."""
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[4]
HERE = Path(__file__).resolve().parent
BASE = 'cdacce8f99c28ca8dba8f5f76831b77b4dfcd836'
METHODS = ['look_state', 'look_alive', 'finish_stall', 'rearm', 'look_failed',
           'look_observations', 'probe_passed', 'save_allowance', 'allowance_row', 'record_progress']
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
    for name, source in [('reference', base), ('current', (ROOT / 'crates/controller/src/looks.rs').read_text())]:
        text = 'use super::*;\ncontroller!();\nimpl Controller {\n'
        text += ''.join(function(source, n, True) for n in METHODS) + '}\n'
        text += ''.join(function(source, n, False) for n in FUNCTIONS)
        (HERE / (name + '.rs')).write_text(text)
    types = base[base.index('#[derive(Clone, Debug, Serialize, Deserialize)]'):base.index('pub(crate) fn resolve_effect_incidents')]
    # The signature's serde boundary remains real; AttachedIncident is unused here.
    sig = base[base.index('#[derive(Clone, Debug, Serialize, Deserialize)]\n#[serde(try_from'):base.index('pub(crate) enum IncidentCause')]
    cause = base[base.index('pub(crate) enum IncidentCause'):base.index('pub(crate) fn incident_needs_rob')]
    (HERE / 'types.rs').write_text(types + sig + cause)


if __name__ == '__main__':
    main()
