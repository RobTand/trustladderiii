"""Copy the G2d arm seam and unchanged delegates from real Git/production source."""
from pathlib import Path
import hashlib
import subprocess

ROOT = Path(__file__).resolve().parents[4]
HERE = Path(__file__).resolve().parent
BASE = 'cf674f1c4e6d5f807d2f03ca3f073c1a7a18b664'
METHODS = ['progress_allowance', 'quota_allowance', 'retire_wait', 'allowance', 'now_ms']


def arms(source):
    start = source.index('            Observation::Progress { key, checkpoint } => {')
    return source[start:source.index('            Observation::Classified {', start)]


def method(source, name):
    start = source.index('fn ' + name + '(')
    start = source.rfind('\n', 0, start) + 1
    return source[start:source.index('\n    }\n', start) + 7]


def state(source):
    start = source.index('#[derive(Clone, Debug, Serialize, Deserialize)]')
    end = source.index('#[derive(Clone, Debug, Serialize, Deserialize)]', start + 1)
    return source[start:end]


def main():
    pins = []
    for side in ['reference', 'current']:
        def read(path):
            if side == 'reference':
                return subprocess.check_output(['git', 'show', BASE + ':' + path], cwd=ROOT).decode()
            return (ROOT / path).read_text()
        arm = arms(read('crates/controller/src/lib.rs'))
        looks = read('crates/controller/src/looks.rs')
        delegates = ''.join(method(looks, name) for name in METHODS)
        text = ('use super::*;\ncontroller!();\nimpl Controller {\n'
                '    fn translate_arm(&mut self, snapshot: &Snapshot, observation: Observation, commit: &mut Commit) -> Result<Option<Event>> {\n'
                '        match observation {\n' + arm +
                '            _ => unreachable!("outside G2d scope"),\n        }\n    }\n' + delegates +
                '}\npub(super) fn run(snapshot: &Snapshot, observation: Observation, commit: &mut Commit) -> Result<Option<Event>> {\n'
                '    Controller::new().translate_arm(snapshot, observation, commit)\n}\n')
        (HERE / (side + '.rs')).write_text(text)
        if side == 'reference':
            for name, content in [('arms', arm), ('delegates', delegates), ('state', state(looks))]:
                (HERE / ('base-' + name + '.rs.txt')).write_text(content)
                pins.append(name + ' ' + hashlib.sha256(content.encode()).hexdigest())
            (HERE / 'types.rs').write_text(state(looks))
    (HERE / 'pins.txt').write_text('base ' + BASE + '\n' + '\n'.join(pins) + '\n')


if __name__ == '__main__':
    main()
