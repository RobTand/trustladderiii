"""Loaded from the read-only harness mount before any checkout imports."""
import json
import os
from pathlib import Path
import sys
import types

import pytest


class Reporter:
    def __init__(self, tests, descriptor, summary=False):
        self.tests = tests
        self.descriptor = descriptor
        self.summary = summary
        self.phases = {}

    def emit(self, **record):
        data = (json.dumps(record, sort_keys=True) + "\n").encode()
        while data:
            data = data[os.write(self.descriptor, data):]

    def pytest_sessionstart(self):
        self.emit(event="start", schema=1)

    @pytest.hookimpl(trylast=True)
    def pytest_collection_modifyitems(self, items):
        if self.summary:
            return
        selected = [item for item in items if item.nodeid in self.tests]
        items[:] = selected
        self.emit(event="collection", tests=[item.nodeid for item in selected])

    def pytest_collectreport(self, report):
        if report.failed:
            if self.summary:
                message = str(report.longrepr)
                self.emit(event="collection-error", test=report.nodeid,
                          message=message[:1000], truncated=len(message) > 1000)
            else:
                self.emit(event="collection-error")

    @pytest.hookimpl(wrapper=True, tryfirst=True)
    def pytest_runtest_makereport(self, item, call):
        report = yield
        assertion = (call.when == "call" and call.excinfo is not None
                     and call.excinfo.errisinstance(AssertionError))
        # Xfail and xpass cannot supply evidence for a named test.
        outcome = "skipped" if hasattr(report, "wasxfail") else report.outcome
        missing = None
        if call.when == "call" and call.excinfo is not None:
            error = call.excinfo.value
            if isinstance(error, AttributeError) and isinstance(error.obj, types.ModuleType):
                source = getattr(error.obj, "__file__", None)
                if source and Path(source).is_relative_to(Path.cwd()):
                    missing = [str(Path(source).relative_to(Path.cwd())), error.name]
        if self.summary:
            message = str(report.longrepr) if report.failed else ""
            self.phases.setdefault(item.nodeid, []).append(
                (report.when, outcome, message[:1000], len(message) > 1000, assertion))
            return report
        self.emit(event="phase", test=item.nodeid, when=report.when,
                  outcome=outcome, assertion=bool(assertion and outcome == "failed"), missing=missing)
        return report

    def pytest_runtest_logreport(self, report):
        if not self.summary or report.when != "teardown":
            return
        phases = self.phases.pop(report.nodeid)
        failures = [phase for phase in phases if phase[1] == "failed"]
        if failures:
            outcome = "failed" if all(p[0] == "call" and p[4] for p in failures) else "error"
        else:
            outcome = "skipped" if any(p[1] == "skipped" for p in phases) else "passed"
        message = "\n".join(p[2] for p in failures)
        self.emit(event="result", test=report.nodeid, outcome=outcome,
                  message=message[:1000], truncated=len(message) > 1000 or any(p[3] for p in failures))

    def pytest_sessionfinish(self, exitstatus):
        self.emit(event="finish", exit_code=int(exitstatus))


def main():
    tests = json.loads(sys.argv[1])
    descriptor = os.dup(1)
    with open(os.devnull, "wb") as sink:
        os.dup2(sink.fileno(), 1)
        os.dup2(sink.fileno(), 2)
    summary = sys.argv[-1] == "--summary"
    arguments = sys.argv[2:-1] if summary else sys.argv[2:]
    checkout = arguments[0] if arguments else "/checkout"
    os.chdir(checkout)
    sys.path.insert(0, str(Path.cwd()))
    if summary:
        return int(pytest.main(["--", *(tests or ["."])], plugins=[Reporter(tests, descriptor, True)]))
    files = sorted({test.split("::")[0] for test in tests if Path(test.split("::")[0]).is_file()})
    if not files:
        reporter = Reporter(tests, descriptor)
        reporter.pytest_sessionstart()
        reporter.emit(event="collection", tests=[])
        reporter.pytest_sessionfinish(5)
        return 5
    return int(pytest.main(files, plugins=[Reporter(tests, descriptor)]))


if __name__ == "__main__":
    raise SystemExit(main())
