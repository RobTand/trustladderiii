"""Prepare private scratch and the checked tokenizer input for remote tests."""
import os
from pathlib import Path
import shutil
import sys
import venv


def prepare():
    os.environ.pop("PYTHONSAFEPATH", None)
    scratch = Path(os.environ['TEST_TMPDIR']).resolve()
    os.environ['HOME'] = str(scratch)
    os.environ['TMPDIR'] = str(scratch)
    cache = scratch / 'data-gym-cache'
    cache.mkdir(exist_ok=True)
    for repository in Path('..').glob('*o200k_base*'):
        source = repository / 'file/fb374d419588a4632f3f557e76b4b70aebbca790'
        shutil.copyfile(source, cache / source.name)
    os.environ['TIKTOKEN_CACHE_DIR'] = str(cache)
    # Python's -I deliberately ignores PYTHONPATH. A real venv with a .pth
    # preserves declared Bazel dependencies for children that clear their env.
    environment = scratch / 'python-environment'
    if not (environment / 'pyvenv.cfg').exists():
        venv.EnvBuilder(with_pip=False, symlinks=True).create(environment)
    site = next(environment.glob('lib/python*/site-packages'))
    paths = [str(Path(path).resolve()) for path in sys.path if path]
    (site / 'bazel-runfiles.pth').write_text('\n'.join(paths) + '\n')
    sys.executable = str(environment / 'bin/python')
    # Light checks put Python 3.14 first on PATH; tests run python3 and python3.14.
    os.environ['PATH'] = os.pathsep.join([str(environment / 'bin'), os.environ.get('PATH', '')])
