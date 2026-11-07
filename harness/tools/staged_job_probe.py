"""Exercise a real staged bundle in an isolated child with no action executable."""
import contextlib
import importlib
import io
import json
import os
from pathlib import Path
import runpy
import sys

def main():
    root = Path(sys.argv[1]).resolve()
    mode = sys.argv[2]
    assert all(name not in os.environ for name in (
        'TL3_ACTION_BINARY', 'RUNFILES_DIR', 'TEST_SRCDIR', 'RUNFILES_MANIFEST_FILE', 'PYTHONPATH'))
    assert not list(root.rglob('tl3-action'))
    sys.path.insert(0, str(root / 'trusted'))
    package = root / 'trusted/tl3_harness'
    modules = []
    for source in sorted(package.glob('*.py')):
        name = 'tl3_harness' + ('.' + source.stem if source.stem != '__init__' else '')
        importlib.import_module(name)
        modules.append(name)

    from tl3_harness import execution, model, proof, redaction, test_tool

    secret = 'TL3FAKE' + ''.join(chr(65 + (index * 7) % 26) for index in range(24))
    text = 'password=' + secret
    redactor = redaction.Redactor(None)
    assert secret not in redactor.window(text, 0, len(text))
    assert secret not in redactor.text(text)
    assert secret not in json.dumps(redactor.payload({'password': secret, 'note': 'ordinary'}))
    assert redactor.clean(redactor.shape({'note': 'ordinary'})) == {'note': 'ordinary'}
    try:
        model.Forcing('fixture', patches=(('app.fn', 'raises', '"AssertionError"'),))
    except model.ProofRefusal as refusal:
        assert refusal.rule == 'forcing.raise_class'
        assert 'RuntimeError' in str(refusal)
        assert refusal.as_json()
    else:
        raise AssertionError('job-side forcing refusal was not enforced')

    assembly = root / 'probe-assembly'
    base, head = assembly / 'base', assembly / 'head'
    base.mkdir(parents=True)
    head.mkdir()
    (head / 'test_probe.py').write_text('def test_probe():\n    assert False\n')
    check = model.Check('job probe', model.Kind.DEMONSTRATING, ('test_probe.py::test_probe',))
    assert proof.assemble_proof(base, head, assembly / 'assembled', check, {'test-files': ['test_*.py']})

    # Only replace the container/admission test seams. The staged runner, report,
    # refusal, supervision and outer action implementations execute unmodified.
    execution.require_admission = lambda: None
    test_tool.require_admission = lambda: None
    runner = package / 'runner.py'
    def command(tree, container, tests):
        return [sys.executable, '-I', '-B', str(runner), json.dumps(tests), str(tree)]
    execution.container_command = command
    test_tool.container_command = command
    os.environ['PRISMABUILD_ACTION_PROGRESS_PATH'] = str(root / 'probe-progress.json')
    os.environ['PRISMABUILD_ACTION_PROGRESS_TOKEN'] = 'fixture'
    os.chdir(root)
    output = io.StringIO()
    if mode == 'pb':
        sys.argv = [str(root / 'action.py'), 'sha256:' + '1' * 64, '[]']
        with contextlib.redirect_stdout(output):
            runpy.run_path(str(root / 'action.py'), run_name='__main__')
        summary = json.loads(next(line.removeprefix(test_tool.PREFIX) for line in output.getvalue().splitlines()
                                  if line.startswith(test_tool.PREFIX)))
        assert summary['status'] == 'completed', summary.get('status')
        assert summary['totals'] == {'passed': 1, 'failed': 1, 'error': 1, 'skipped': 0}, summary['totals']
    else:
        assert mode == 'checks'
        with contextlib.redirect_stdout(output):
            runpy.run_path(str(root / 'action.py'), run_name='__main__')
        report = json.loads(next(line.removeprefix('TL3_CHECKS ') for line in output.getvalue().splitlines()
                                 if line.startswith('TL3_CHECKS ')))
        assert report['results'] and all(item['verdict'] == 'pass' for item in report['results']), report['results']
    print(json.dumps({'modules': modules, 'mode': mode, 'entrypoints': ['Redactor.window', 'Redactor.text',
        'Redactor.payload', 'Redactor.shape/clean', 'Forcing', 'assemble_proof', 'action.py', 'runner.py']}))


if __name__ == "__main__":
    main()
