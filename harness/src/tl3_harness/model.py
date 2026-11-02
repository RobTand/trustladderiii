from dataclasses import dataclass, field
from enum import StrEnum


class HarnessInputError(ValueError):
    """TL3's own check input is malformed: TL3's error, never the repo's or PrismaBuild's (Q51)."""


class Kind(StrEnum):
    DEMONSTRATING = "demonstrating"
    REGRESSION = "regression"
    BEHAVIOR_PRESERVING = "behavior-preserving"
    REPRODUCING = "reproducing"


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
class Failure:
    """What a failed test showed: the phase, the exception (type and first line), and the report text."""
    phase: str
    exception: str
    message: str


class FailureNotes(dict):
    """Failure messages by test, plus the `Failure` behind each, for a run whose evidence `judge` quotes."""

    def __init__(self):
        super().__init__()
        self.details: dict[str, Failure] = {}


@dataclass(frozen=True)
class Forcing:
    """A plan's declarative recreation of a flake's condition (#393)."""
    symptom: str
    environment: tuple[tuple[str, str], ...] = ()
    patches: tuple[tuple[str, str, str], ...] = ()

    @classmethod
    def parse(cls, data) -> "Forcing":
        raise NotImplementedError

    def as_json(self) -> dict:
        raise NotImplementedError


@dataclass(frozen=True)
class Check:
    name: str
    kind: Kind
    tests: tuple[str, ...]
    forcing: Forcing | None = None

    def __post_init__(self):
        if not self.name or not isinstance(self.kind, Kind) or not self.tests:
            raise HarnessInputError("a check needs a name, a kind and named tests")
        if len(set(self.tests)) != len(self.tests):
            raise HarnessInputError("a check repeats a test")
        for test in self.tests:
            path, *names = test.split("::")
            if (not names or not all(names) or not path.endswith(".py")
                    or path.startswith(("/", "-")) or ".." in path.split("/")
                    or "\\" in path or "\x00" in test):
                raise HarnessInputError(f"not an exact pytest node id: {test!r}")


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


MESSAGE_LIMIT = 1000
TRUNCATED = " [truncated]"
WHEN = {"setup": "in setup", "call": "during the call", "teardown": "during teardown"}


def base_refusal(check: Check, test: str, before: Outcome, notes: FailureNotes | None) -> str:
    """Why a demonstrating or reproducing test's base run did not count; empty when it did (Q40, Q42)."""
    verb = "demonstrate" if check.kind == Kind.DEMONSTRATING else "reproduce"
    if before == Outcome.PASSED:
        return (f"This test passed on the base. A {check.kind} test must fail by an assertion on the base, "
                f"or lack a name this change adds, so it does not {verb} the change. "
                "Make it assert the behavior the change adds.")
    failure = (notes.details if notes is not None else {}).get(test)
    if before != Outcome.ERROR or failure is None:
        return ""
    what = f"`{failure.exception}`" if failure.exception else "an error"
    match failure.phase:
        case "collection":
            return (f"On the base this test file could not be collected: {what}. An import failure counts only "
                    f"when it names a module or name this change adds, and this one does not, so the test does "
                    f"not {verb} the change. Import the new name inside the test body, or make the base run "
                    "fail by an assertion.")
        case "setup":
            return (f"On the base this test failed in setup with {what}. A setup or fixture error is not an "
                    f"assertion failure, so it does not {verb} the change. "
                    "Make the base run fail by an assertion in the test body.")
        case phase:
            return (f"On the base this test raised {what} {WHEN.get(phase, 'in the test')}. That is neither an "
                    f"assertion failure nor a missing name this change adds, so it does not {verb} the change. "
                    "Make the base run fail by an assertion, or import the new name inside the test body.")


def judge(check: Check, base: dict[str, Outcome], head: dict[str, Outcome], *,
          messages: dict[str, str] | None = None, base_notes: FailureNotes | None = None) -> Result:
    tests = []
    infrastructure = False
    for test in check.tests:
        before, after = base[test], head[test]
        infrastructure |= Outcome.NO_REPORT in (before, after)
        pre_existing = (check.kind == Kind.REGRESSION
                        and before in (Outcome.ASSERTION, Outcome.ERROR))
        match check.kind:
            case Kind.DEMONSTRATING | Kind.REPRODUCING:
                passed = before in (Outcome.ASSERTION, Outcome.ADDED_NAME) and after == Outcome.PASSED
            case Kind.REGRESSION:
                available = before not in (Outcome.SKIPPED, Outcome.MISSING, Outcome.UNCOLLECTED, Outcome.NO_REPORT)
                passed = available and (after == Outcome.PASSED or
                                         pre_existing and after in (Outcome.ASSERTION, Outcome.ERROR))
            case Kind.BEHAVIOR_PRESERVING:
                passed = before == after == Outcome.PASSED
        message = "" if passed else (messages or {}).get(test, "")
        if not passed and check.kind in (Kind.DEMONSTRATING, Kind.REPRODUCING) and (
                refusal := base_refusal(check, test, before, base_notes)):
            message = refusal + (f"\nOn the head: {message}" if message else "")
            if len(message) > MESSAGE_LIMIT:
                message = message[:MESSAGE_LIMIT - len(TRUNCATED)] + TRUNCATED
        if not passed and not message:
            message = f"Check {check.kind}: base {before}, head {after}."
        tests.append(TestResult(test, before, after, pre_existing, passed, message[:MESSAGE_LIMIT]))
    verdict = (Verdict.INFRASTRUCTURE_FAILURE if infrastructure else
               Verdict.PASS if all(test.passed for test in tests) else Verdict.FAIL)
    return Result(check.name, check.kind, verdict, tuple(tests))
