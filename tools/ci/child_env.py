"""Allow only runtime configuration into non-Bazel CI children.

Bazel authentication stays with the gate and Bazel. Never forward credentials
through --test_env or --action_env.
"""
import os
import sys

# Keep this list explicit. A new runner setting must be reviewed before a test
# or tool can inherit it. In particular, do not allow arbitrary CI/GITHUB vars.
RUNTIME_KEYS = frozenset("""
PATH HOME USER LOGNAME SHELL TMPDIR TMP TEMP LANG LANGUAGE LC_ALL LC_CTYPE TZ
SYSTEMROOT WINDIR COMSPEC PATHEXT
PYTHONPATH PYTHONHOME PYTHONUTF8 PYTHONIOENCODING PYTHONNOUSERSITE
PYTHONDONTWRITEBYTECODE PYTEST_DISABLE_PLUGIN_AUTOLOAD
VIRTUAL_ENV UV_CACHE_DIR UV_PYTHON_INSTALL_DIR UV_PYTHON_PREFERENCE
CARGO CARGO_HOME RUSTUP_HOME RUSTC RUSTC_BOOTSTRAP CARGO_BUILD_JOBS
RUST_TEST_THREADS RUST_BACKTRACE RUST_LOG RUSTFLAGS CARGO_ENCODED_RUSTFLAGS
LD_LIBRARY_PATH DYLD_LIBRARY_PATH LIBRARY_PATH PKG_CONFIG_PATH
CC CXX AR CFLAGS CXXFLAGS LDFLAGS
OMP_NUM_THREADS MKL_NUM_THREADS OPENBLAS_NUM_THREADS RAYON_NUM_THREADS
TOKIO_WORKER_THREADS TIKTOKEN_CACHE_DIR DATA_GYM_CACHE_DIR
RUNNER_ENVIRONMENT RUNNER_OS RUNNER_ARCH CI BUILDBUDDY_ARTIFACTS_DIRECTORY
GIT_CONFIG_NOSYSTEM GIT_CONFIG_GLOBAL GIT_CONFIG_SYSTEM GIT_TERMINAL_PROMPT
GITHUB_EVENT_PATH
TL3_BINARY TL3_DEPLOY_HELPER TL3_TEST_UV TL3_TEST_POSTGRES_DIR TL3_RUNNER_CLASS TL3_BUDGETS
""".split())


def child_env(source=None):
    source = os.environ if source is None else source
    return {name: value for name, value in source.items()
            if (name in RUNTIME_KEYS or name.startswith(('TEST_', 'RUNFILES_'))
                or name in {'JAVA_RUNFILES', 'PYTHON_RUNFILES'})
            and not name.upper().endswith(('_API_KEY', '_TOKEN'))}


if __name__ == '__main__':
    # Workflow commands use the same policy as Python subprocesses.
    os.execvpe(sys.argv[1], sys.argv[1:], child_env())
