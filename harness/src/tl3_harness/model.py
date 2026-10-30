from dataclasses import dataclass, field
from enum import StrEnum


class Kind(StrEnum):
    DEMONSTRATING = "demonstrating"
    REGRESSION = "regression"
    BEHAVIOR_PRESERVING = "behavior-preserving"


class Outcome(StrEnum):
    PASSED = "passed"
    ASSERTION = "assertion"
    ADDED_NAME = "added-name"
    ERROR = "error"
    SKIPPED = "skipped"
    MISSING = "missing"
    UNCOLLECTED = "uncollected"
    NO_REPORT = "no-report"


class Custody(StrEnum):
    IN_PROCESS = "in-process"


class Verdict(StrEnum):
    PASS = "pass"
    FAIL = "fail"
    INFRASTRUCTURE_FAILURE = "infrastructure-failure"


@dataclass(frozen=True)
class Check:
    name: str
    kind: Kind
    tests: tuple[str, ...]

    def __post_init__(self):
        if not self.name or not isinstance(self.kind, Kind) or not self.tests:
            raise ValueError("a check needs a name, a kind and named tests")
        if len(set(self.tests)) != len(self.tests):
            raise ValueError("a check repeats a test")
        for test in self.tests:
            path, *names = test.split("::")
            if (not names or not all(names) or not path.endswith(".py")
                    or path.startswith(("/", "-")) or ".." in path.split("/")
                    or "\\" in path or "\x00" in test):
                raise ValueError(f"not an exact pytest node id: {test!r}")


@dataclass(frozen=True)
class TestResult:
    test: str
    base: Outcome
    head: Outcome
    pre_existing: bool
    passed: bool
    message: str = ""


@dataclass(frozen=True)
class Result:
    name: str
    kind: Kind
    verdict: Verdict
    tests: tuple[TestResult, ...]
    custody: Custody = field(default=Custody.IN_PROCESS, init=False)


def judge(check: Check, base: dict[str, Outcome], head: dict[str, Outcome], *,
          messages: dict[str, str] | None = None) -> Result:
    tests = []
    infrastructure = False
    for test in check.tests:
        before, after = base[test], head[test]
        infrastructure |= Outcome.NO_REPORT in (before, after)
        pre_existing = (check.kind == Kind.REGRESSION
                        and before in (Outcome.ASSERTION, Outcome.ERROR))
        match check.kind:
            case Kind.DEMONSTRATING:
                passed = before in (Outcome.ASSERTION, Outcome.ADDED_NAME) and after == Outcome.PASSED
            case Kind.REGRESSION:
                available = before not in (Outcome.SKIPPED, Outcome.MISSING, Outcome.UNCOLLECTED, Outcome.NO_REPORT)
                passed = available and (after == Outcome.PASSED or
                                         pre_existing and after in (Outcome.ASSERTION, Outcome.ERROR))
            case Kind.BEHAVIOR_PRESERVING:
                passed = before == after == Outcome.PASSED
        message = "" if passed else (messages or {}).get(test, "")
        if not passed and not message:
            message = f"Check {check.kind}: base {before}, head {after}."
        tests.append(TestResult(test, before, after, pre_existing, passed, message[:1000]))
    verdict = (Verdict.INFRASTRUCTURE_FAILURE if infrastructure else
               Verdict.PASS if all(test.passed for test in tests) else Verdict.FAIL)
    return Result(check.name, check.kind, verdict, tuple(tests))
