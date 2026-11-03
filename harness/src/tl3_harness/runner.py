"""Loaded from the read-only harness mount before any checkout imports."""
import ast
import builtins
import copy
import importlib
import importlib.util
import json
import os
import shutil
from pathlib import Path
import sys
import tempfile
import time
import types

import pytest

spec = importlib.util.spec_from_file_location("tl3_node_ids", Path(__file__).with_name("node_ids.py"))
node_ids = importlib.util.module_from_spec(spec)
spec.loader.exec_module(node_ids)
selects = node_ids.selects
spec = importlib.util.spec_from_file_location("tl3_forcing_ownership", Path(__file__).with_name("forcing_ownership.py"))
ownership = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ownership)
ForcingError = ownership.ForcingError
spec = importlib.util.spec_from_file_location("tl3_limits", Path(__file__).with_name("limits.py"))
limits = importlib.util.module_from_spec(spec)
spec.loader.exec_module(limits)


def module_candidates(module):
    root = Path.cwd()
    candidates = []
    for entry in sys.path:
        directory = Path(entry).resolve()
        if directory.is_relative_to(root):
            source = directory.joinpath(*module.split('.'))
            candidates.extend([str(source.relative_to(root)) + suffix, ""]
                              for suffix in (".py", "/__init__.py"))
    return candidates


def missing_name(error):
    root = Path.cwd()
    # Pytest wraps import failures in Collector.CollectError during collection.
    while error is not None and not isinstance(error, (ImportError, AttributeError)):
        error = error.__cause__ or error.__context__
    if isinstance(error, AttributeError) and isinstance(error.obj, types.ModuleType):
        source = getattr(error.obj, "__file__", None)
        if source and Path(source).is_relative_to(root):
            return [str(Path(source).relative_to(root)), error.name]
    if isinstance(error, ModuleNotFoundError) and error.name:
        # The host matches these candidates against committed additions, never error text.
        return module_candidates(error.name) or None
    if isinstance(error, ImportError) and error.name and error.path:
        source = Path(error.path)
        if source.is_relative_to(root):
            # ImportError.name is the module, not the missing symbol. Read the
            # failing import statement rather than interpreting Python's error message.
            traceback, failing = error.__traceback__, None
            while traceback:
                frame = traceback.tb_frame
                filename = Path(frame.f_code.co_filename)
                if filename.is_file() and filename.is_relative_to(root):
                    failing = traceback
                traceback = traceback.tb_next
            if failing is None:
                return None
            try:
                module = ast.parse(Path(failing.tb_frame.f_code.co_filename).read_bytes())
                imports = [node for node in ast.walk(module) if isinstance(node, ast.ImportFrom)
                           and node.lineno <= failing.tb_lineno <= node.end_lineno]
                if len(imports) != 1:
                    return None
                node = imports[0]
                imported = node.module
                if node.level:
                    imported = importlib.util.resolve_name(
                        '.' * node.level + (imported or ''),
                        failing.tb_frame.f_globals.get('__package__'))
                loaded = sys.modules.get(error.name)
                if imported != error.name or loaded is None:
                    return None
                for alias in node.names:
                    if not hasattr(loaded, alias.name):
                        candidates = [[str(source.relative_to(root)), alias.name]]
                        if source.name == '__init__.py':
                            child = source.parent / alias.name
                            candidates.extend([str(child.relative_to(root)) + suffix, ""]
                                              for suffix in (".py", "/__init__.py"))
                        return candidates
            except (SyntaxError, OSError, ImportError, ValueError):
                pass
    return None


def forcing_owner(target):
    """The object holding the target's attribute. It must be a module whose file is in the
    checkout, or a class defined in one, so a forcing can't patch pytest, the standard
    library or this reporter."""
    root = Path.cwd()
    parts = target.split(".")
    for split in range(len(parts) - 1, 0, -1):
        try:
            owner = importlib.import_module(".".join(parts[:split]))
        except ModuleNotFoundError as error:
            if error.name and (error.name == ".".join(parts[:split]) or ".".join(parts[:split]).startswith(error.name + ".")):
                continue
            raise ForcingError(target=target, module=".".join(parts[:split]), reason="import_failure") from error
        except Exception as error:
            raise ForcingError(target=target, module=".".join(parts[:split]), reason="import_failure") from error
        break
    else:
        raise ForcingError(target=target, reason="module_not_found")
    for name in parts[split:-1]:
        if not hasattr(owner, name):
            raise ForcingError(target=target, module=getattr(owner, "__name__", "unknown"), reason="missing_attribute")
        owner = getattr(owner, name)
    ownership.repository_owner(target, owner, root)
    return owner, parts[-1]


# A forcing's delays add up to at most this per test; later calls run undelayed.
MAX_TOTAL_DELAY_SECONDS = 10


def forced_value(owner, name, mode, argument, sleep=time.sleep, budget=None, counter=None):
    if mode == "value":
        return argument
    if mode == "returns":
        return lambda *args, **kwargs: copy.deepcopy(argument)
    if mode == "raises":
        error = getattr(builtins, argument, None)
        if not (isinstance(error, type) and issubclass(error, Exception) and not issubclass(error, AssertionError)):
            raise ForcingError(f"a forcing can't raise {argument}")

        def raising(*args, **kwargs):
            raise error("forced by the plan")
        return raising
    original = getattr(owner, name)
    if not callable(original):
        raise ForcingError(f"delay_seconds needs a callable target, not {type(original).__name__}")

    budget = [MAX_TOTAL_DELAY_SECONDS] if budget is None else budget

    def delayed(*args, **kwargs):
        if counter is not None:
            counter[0] += 1
        wait = min(argument, budget[0])
        budget[0] -= wait
        if wait:
            sleep(wait)
        return original(*args, **kwargs)
    return delayed


def describe(error):
    """`Type: first line` of the exception, kept apart from the traceback the 1,000-char limit cuts."""
    if isinstance(error, pytest.Collector.CollectError) and error.__cause__ is not None:
        error = error.__cause__
    line = next((line.strip() for line in str(error).splitlines() if line.strip()), "")
    return (f"{type(error).__name__}: {line}" if line else type(error).__name__).replace("`", "'")[:200]


class Reporter:
    def __init__(self, tests, descriptor, summary=False, parallel=False, patches=(), selection=None):
        self.tests = tests
        self.descriptor = descriptor
        self.summary = summary
        # Under xdist, workers run the tests and this plugin sees only their reports.
        self.parallel = parallel
        self.patches = patches
        self.selection = selection
        self.target_hits = {}
        self.forcing_refusals = {}
        self.phases = {}
        self.collection_errors = []
        self.reported = set()
        self.started = False

    @pytest.fixture(autouse=True)
    def _tl3_scratch(self, tmp_path):
        previous = tempfile.tempdir
        environment = {name: os.environ.get(name) for name in ('TMPDIR', 'TEMP', 'TMP')}
        tempfile.tempdir = str(tmp_path)
        os.environ.update(dict.fromkeys(environment, str(tmp_path)))
        try:
            yield
        finally:
            tempfile.tempdir = previous
            for name, value in environment.items():
                if value is None:
                    os.environ.pop(name, None)
                else:
                    os.environ[name] = value
            # A writer can race removal. Preserve the test's outcome; the
            # container's fixed tmpfs still bounds and discards any residue.
            shutil.rmtree(tmp_path, ignore_errors=True)

    @pytest.fixture(autouse=True)
    def _tl3_forcing(self, request):
        # Applied before the test's own fixtures and undone after them, so the test's own
        # monkeypatches take precedence while it runs (#393).
        budget = [MAX_TOTAL_DELAY_SECONDS]
        hits = self.target_hits.setdefault(request.node.nodeid, {})
        with pytest.MonkeyPatch.context() as patch:
            for forcing in self.patches:
                target = forcing["target"]
                (mode, argument), = ((key, value) for key, value in forcing.items() if key != "target")
                try:
                    owner, name = forcing_owner(target)
                except ForcingError as error:
                    if error.refusal is not None:
                        self.forcing_refusals[request.node.nodeid] = error.refusal
                    raise
                counter = hits.setdefault(target, [0]) if mode == "delay_seconds" else None
                patch.setattr(owner, name, forced_value(owner, name, mode, argument, budget=budget, counter=counter))
            yield

    def emit(self, **record):
        data = (json.dumps(record, sort_keys=True, ensure_ascii=False) + "\n").encode(errors='backslashreplace')
        while data:
            data = data[os.write(self.descriptor, data):]

    def pytest_sessionstart(self):
        if not self.started:
            self.started = True
            self.emit(event="start", schema=1)

    @pytest.hookimpl(trylast=True)
    def pytest_collection_modifyitems(self, items, config, session):
        if self.summary:
            return
        selected = [item for item in items if any(selects(test, item.nodeid) for test in self.tests)]
        try:
            saturation = None
            if self.selection is not None:
                if config.option.keyword or config.option.markexpr or config.option.deselect:
                    raise ValueError("Selected execution groups require complete modules; remove pytest deselection options.")
                remaining = self.selection["limit"]
                admitted = []
                for group in self.selection["groups"]:
                    members = [item for item in selected if item.nodeid.split("::", 1)[0] in group]
                    names = [item.nodeid for item in members]
                    if len(names) > remaining:
                        if self.selection.get("prefix") and (admitted or self.selection.get("allow_empty")):
                            saturation = {"unit": group, "cases": len(names), "remaining": remaining}
                            break
                        raise ValueError(f"Selected execution group {group} needs {len(names)} tests; "
                                         f"remaining selected-test budget is {remaining}.")
                    remaining -= len(names)
                    expected = self.selection["expected"]
                    if expected is not None and names != [name for name in expected if name.split("::", 1)[0] in group]:
                        raise ValueError(f"Selected collection changed for execution group {group}; "
                                         "base and head must have identical complete membership.")
                    admitted.extend(members)
                selected = admitted
                if saturation:
                    blocked = self.selection["groups"].index(saturation["unit"])
                    skipped_files = {member for group in self.selection["groups"][blocked:] for member in group}
                    discarded = [report for report in self.collection_errors
                                 if report.nodeid.split("::", 1)[0] in skipped_files]
                    # Pytest counted these collection failures before admission chose its
                    # prefix. Unchecked suffix files cannot change that prefix's outcome.
                    session.testsfailed -= len(discarded)
                    self.collection_errors[:] = [report for report in self.collection_errors if report not in discarded]
            limits.validate_plan_size(item.nodeid for item in selected)
        except ValueError as error:
            self.collection_errors.clear()
            items.clear()
            self.emit(event="plan-error", message=str(error)[:1000])
            self.emit(event="collection", tests=[])
            pytest.exit(str(error), returncode=4)
        self.emit_collection_errors()
        items[:] = selected
        self.emit(event="collection", tests=[item.nodeid for item in selected],
                  **({"saturation": saturation} if saturation else {}))

    def pytest_exception_interact(self, node, call, report):
        report.tl3_missing = missing_name(call.excinfo.value)
        report.tl3_exception = describe(call.excinfo.value)

    def pytest_collectreport(self, report):
        if report.failed:
            if self.summary:
                # Each xdist worker collects, so the same error can arrive more than once.
                if report.nodeid in self.reported:
                    return
                self.reported.add(report.nodeid)
                message = str(report.longrepr)
                self.emit(event="collection-error", test=report.nodeid,
                          message=message[:1000], truncated=len(message) > 1000)
            else:
                self.collection_errors.append(report)

    def emit_collection_errors(self):
        # pytest exposes the exception after its collection report hook.
        for report in self.collection_errors:
            self.emit(event="collection-error", test=report.nodeid,
                      missing=getattr(report, "tl3_missing", None),
                      exception=getattr(report, "tl3_exception", ""),
                      message=str(report.longrepr)[:1000])
        self.collection_errors.clear()

    @pytest.hookimpl(wrapper=True, tryfirst=True)
    def pytest_runtest_makereport(self, item, call):
        report = yield
        assertion = (call.when == "call" and call.excinfo is not None
                     and call.excinfo.errisinstance((AssertionError, pytest.fail.Exception)))
        # Xfail and xpass cannot supply evidence for a named test.
        outcome = "skipped" if hasattr(report, "wasxfail") else report.outcome
        missing = missing_name(call.excinfo.value) if call.when == "call" and call.excinfo else None
        if self.summary:
            message = str(report.longrepr) if report.failed else ""
            self.phases.setdefault(item.nodeid, []).append(
                (report.when, outcome, message[:1000], len(message) > 1000, assertion))
            return report
        self.emit(event="phase", test=item.nodeid, when=report.when,
                  outcome=outcome, assertion=bool(assertion and outcome == "failed"), missing=missing,
                  exception=describe(call.excinfo.value) if report.failed and call.excinfo else "",
                  message=str(report.longrepr)[:1000] if report.failed else "")
        return report

    def pytest_runtest_logreport(self, report):
        if not self.summary:
            return
        if self.parallel:
            # makereport ran in a worker. Its crash line tells an assertion from an error.
            crash = getattr(getattr(report.longrepr, "reprcrash", None), "message", "") or ""
            assertion = (report.when == "call" and report.failed
                         and crash.startswith(("AssertionError", "assert ", "Failed")))
            outcome = "skipped" if hasattr(report, "wasxfail") else report.outcome
            message = str(report.longrepr) if report.failed else ""
            self.phases.setdefault(report.nodeid, []).append(
                (report.when, outcome, message[:1000], len(message) > 1000, assertion))
        # xdist reports a crashed worker's test once, outside the three phases.
        final = report.when == "teardown" or self.parallel and report.when not in ("setup", "call")
        if not final:
            return
        phases = self.phases.pop(report.nodeid)
        failures = [phase for phase in phases if phase[1] == "failed"]
        if failures:
            outcome = "failed" if all(p[0] == "call" and p[4] for p in failures) else "error"
        else:
            outcome = "skipped" if any(p[1] == "skipped" for p in phases) else "passed"
        message = "\n".join(p[2] for p in failures)
        hits = self.target_hits.pop(report.nodeid, {})
        extra = {"target_hits": {target: count[0] for target, count in hits.items()}} if self.patches else {}
        refusal = self.forcing_refusals.pop(report.nodeid, None)
        if refusal is not None:
            extra['forcing_refusal'] = refusal
        self.emit(event="result", test=report.nodeid, outcome=outcome, **extra,
                  message=message[:1000], truncated=len(message) > 1000 or any(p[3] for p in failures))

    def pytest_sessionfinish(self, exitstatus):
        self.emit_collection_errors()
        self.emit(event="finish", exit_code=int(exitstatus))


def main():
    tests = json.load(sys.stdin) if sys.argv[1] == "-" else json.loads(sys.argv[1])
    forcing = {}
    selection = None
    if isinstance(tests, dict):
        # A forced check's selection carries the plan's forcing, which the host validated (#393).
        selection = tests.get("selection")
        tests, forcing = tests["tests"], tests.get("forcing", {})
        os.environ.update(forcing.get("environment", {}))
    descriptor = os.dup(1)
    with open(os.devnull, "wb") as sink:
        os.dup2(sink.fileno(), 1)
        os.dup2(sink.fileno(), 2)
    flags = [argument for argument in sys.argv[2:] if argument.startswith("--")]
    arguments = [argument for argument in sys.argv[2:] if not argument.startswith("--")]
    summary = "--summary" in flags
    parallel = next((int(flag.split("=", 1)[1]) for flag in flags if flag.startswith("--parallel=")), 0)
    checkout = arguments[0] if arguments else "/checkout"
    os.chdir(checkout)
    sys.path.insert(0, str(Path.cwd()))
    reporter = Reporter(tests, descriptor, summary, parallel=bool(summary and parallel),
                        patches=tuple(forcing.get("patches", ())), selection=selection)
    # Start before pytest loads the checkout's conftest files, ini options and
    # plugins. Any exit after this point may come from the code under test.
    reporter.pytest_sessionstart()
    if summary and parallel:
        # The image supplies xdist. Load it once, by name, in this process and its workers,
        # as the container's disabled plugin autoload already requires.
        os.environ["PYTEST_DISABLE_PLUGIN_AUTOLOAD"] = "1"
        if importlib.util.find_spec("xdist") is None:
            reporter.emit(event="unavailable", message="pytest-xdist is not installed in the test image")
            reporter.pytest_sessionfinish(0)
            return 0
        return int(pytest.main(["--rootdir=.", "-p", "xdist.plugin", "-n", str(parallel),
                                "--", *(tests or ["."])], plugins=[reporter]))
    if summary:
        return int(pytest.main(["--rootdir=.", "--", *(tests or ["."])], plugins=[reporter]))
    files = sorted({test.split("::")[0] for test in tests if Path(test.split("::")[0]).is_file()})
    if not files:
        reporter.emit(event="collection", tests=[])
        reporter.pytest_sessionfinish(5)
        return 5
    return int(pytest.main(["--rootdir=.", "--continue-on-collection-errors", *files], plugins=[reporter]))


if __name__ == "__main__":
    raise SystemExit(main())
