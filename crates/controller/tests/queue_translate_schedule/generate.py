"""Refresh executable copies; source-identity tests reject stale copies."""
from pathlib import Path
import subprocess

BASE = 'cdacce8f99c28ca8dba8f5f76831b77b4dfcd836'
HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[3]


def function(text, name):
    import re
    match = re.search(r'^    (?:pub(?:\(crate\))? )?fn ' + name + r'\(', text, re.M)
    if match is None:
        raise RuntimeError(f'missing function {name}')
    end = text.index('\n    }\n', match.start()) + 7
    return text[match.start():end]


def projection(text):
    translate = function(text, 'translate')
    preamble = translate[translate.index('        let mut ci_details'):translate.index('        let event = match observation {')]
    arm = translate[translate.index('            Observation::Queue(mut fact) => {'):translate.index('            Observation::GitHubComment {', translate.index('            Observation::Queue(mut fact) => {'))]
    tail = translate[translate.index('        let finding = match event {'):]
    signature = translate[:translate.index('        if let Observation::GitHubComment')]
    return signature.replace('    fn translate(', '    pub(super) fn translate(') + preamble + '        let event = match observation {\n' + arm + '            _ => return Ok(None),\n        };\n' + tail


def render(text):
    return 'use super::*;\nimpl Shell {\n' + '\n'.join(function(text, name) for name in ['observe', 'queue_observation']) + '\n' + projection(text) + '\n}\n'


def main():
    base = subprocess.check_output(['git', 'show', f'{BASE}:crates/controller/src/lib.rs'], text=True)
    for name in ['observe', 'queue_observation', 'translate']:
        (HERE / f'base-{name}.rs.txt').write_text(function(base, name))
    (HERE / 'reference.rs').write_text(render(base))
    (HERE / 'current.rs').write_text(render((ROOT / 'crates/controller/src/lib.rs').read_text()))
    adapter = subprocess.check_output(['git', 'show', f'{BASE}:crates/controller/src/adapters/mergify.rs'], text=True)
    methods = ['node', 'setup', 'recorded', 'identity']
    text = 'use super::*;\nuse adapters::mergify::Evidence;\nimpl QueueFactExt for adapters::mergify::QueueFact {\n'
    for name in methods:
        text += function(adapter, name).replace('pub(crate) fn', 'fn') + '\n'
    (HERE / 'queue_fact.rs').write_text(text + '}\n')
    (HERE / 'base-adapter.rs.txt').write_text(adapter)
    findings = (ROOT / 'crates/controller/src/findings.rs').read_text()
    (HERE / 'findings.rs').write_text('use serde::Serialize;\nuse tl3_controller::findings::{CiFailure, FailedTest};\npub use tl3_controller::findings::{QueueCiEvidence};\n' + findings[findings.index('#[derive(Serialize)]'):findings.index('pub(crate) struct CurrentReview')].replace('crate::contracts::MESSAGE_CHARS', 'tl3_controller::contracts::MESSAGE_CHARS'))
    ci = (ROOT / 'crates/controller/src/adapters/mergify/ci_evidence.rs').read_text()
    (HERE / 'ci.rs').write_text('use super::*;\nuse findings::QueueCiEvidence;\n' + ci[ci.index('fn terminal_text'):ci.index('fn failures(')] + ci[ci.index('fn test_path'):ci.index('\n#[cfg(test)]')])


if __name__ == '__main__':
    main()
