"""Replay small controller mutations inside an admitted PrismaBuild action.

Never run this against the live controller checkout. Every mutation restores its
source before the next build. A panic from an assertion kills a mutant; a build
failure, fixture timeout, or unrelated process failure never counts.
"""
import argparse
import json
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[2]
STEERING = 'crates/controller/src/steering.rs'
WRITER = 'crates/controller/src/adapters/workers/writer.rs'
MUTANTS = {
    'pause': (STEERING, '"pause" => Command::Pause,', '"pause" => Command::Resume,',
              ['commands', 'replacement', 'queued_pause', 'classifier_failures',
               'concurrent_requests', 'answer_replacement']),
    'clarify': (STEERING, '"unclear" => Answer::Unclear,', '"unclear" => Answer::Stop,', ['answers']),
    'continue': (STEERING, '"continue" => Answer::Continue,', '"continue" => Answer::Stop,', ['answer_recovery']),
    'guidance': (STEERING, '.push(reply["guidance"].clone());', '.push(json!("Wrong guidance"));', ['guidance']),
    'credit': (STEERING, 'Answer::AddCredit(credit_amount(&reply)?)',
               'Answer::AddCredit(credit_amount(&reply)?.saturating_add(1))', ['credit']),
    'revert': ('crates/controller/src/adapters/github.rs',
               'The owner requested a revert as part of this correction.',
               'The owner requested a correction.', ['correction']),
    'writer-policy': (WRITER, 'match checked {',
                      'match checked.or_else(|_| Ok("mutation accepted".to_owned())) {',
                      ['writer:wrong-quote', 'writer:no-draft', 'writer:recommendation']),
    'writer-shape': (WRITER, 'if input.kind == MessageKind::Clarification\n            &&',
                     'if false && input.kind == MessageKind::Clarification\n            &&',
                     ['writer:no-excerpt', 'writer:no-text', 'writer:always-multiline']),
    'exit': ('crates/controller/src/adapters/workers.rs', 'if exit["code"] != 0 {',
             'if false && exit["code"] != 0 {', ['workers:wire']),
}


def build(target):
    command = ['cargo', 'test', '--locked', '-p', 'tl3-controller', '--test', target,
               '--no-run', '--message-format=json']
    output = subprocess.check_output(command, cwd=ROOT, text=True)
    binaries = [event['executable'] for line in output.splitlines()
                if (event := json.loads(line)).get('reason') == 'compiler-artifact'
                and event['target']['name'] == target and event['executable']]
    if len(binaries) != 1:
        raise RuntimeError(f'Missing unique {target} test binary')
    return binaries[0]


def run_mutant(name):
    file, old, new, cases = MUTANTS[name]
    path = ROOT / file
    original = path.read_text()
    if original.count(old) != 1:
        raise RuntimeError(f'{name}: mutation target moved')
    try:
        path.write_text(original.replace(old, new))
        target = 'workers' if cases[0].startswith('workers:') else 'github'
        binary = build(target)
        for case in cases:
            flag = ('--gate-wire' if case == 'workers:wire' else
                    '--writer-structural=' + case.split(':')[1] if case.startswith('writer:') else
                    '--gate-case=' + case)
            with tempfile.TemporaryFile(mode='w+') as log:
                result = subprocess.run([binary, flag], cwd=ROOT, stdout=log,
                                        stderr=subprocess.STDOUT, timeout=120)
                log.seek(0)
                output = log.read()
            killed = (result.returncode != 0 and 'panicked at' in output
                      and ('assertion' in output or 'assert!' in output)
                      and 'fixture did not' not in output and 'fixture did not finish' not in output)
            print(json.dumps({'mutant': name, 'test': case, 'returncode': result.returncode,
                              'killed_by_assertion': killed, 'failure': output[-4000:]}), flush=True)
            if not killed:
                raise RuntimeError(f'{name}/{case} did not kill the mutant with an assertion')
    finally:
        path.write_text(original)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('mutant', choices=['all', *MUTANTS])
    args = parser.parse_args()
    subprocess.run(['uv', 'sync', '--locked', '--python', '3.14+gil', '--project', 'workers'],
                   cwd=ROOT, check=True)
    for name in MUTANTS if args.mutant == 'all' else [args.mutant]:
        run_mutant(name)


if __name__ == '__main__':
    main()
