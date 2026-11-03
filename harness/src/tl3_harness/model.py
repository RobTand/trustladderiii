import builtins
from dataclasses import dataclass, field
from enum import StrEnum
import json
import re


ALWAYS_CHECK = "Repository always tests"


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


FORCING_LIMIT = 8
FORCING_TEXT = 1000
FORCING_LITERAL_BYTES = 4096
MAX_DELAY_SECONDS = 5
PATCH_MODES = ("value", "returns", "raises", "delay_seconds")
NATIVE_THREAD_ENVIRONMENT = {"OMP_NUM_THREADS": "1", "MKL_NUM_THREADS": "1",
                             "OPENBLAS_NUM_THREADS": "1"}
# Variables the harness itself sets or the interpreter reads; a forcing can't override them.
RESERVED_ENVIRONMENT = frozenset({"HOME", "PATH", "TMPDIR", "TEMP", "TMP", *NATIVE_THREAD_ENVIRONMENT})
RESERVED_PREFIXES = ("PYTHON", "PYTEST", "LD_", "TL3_", "PRISMABUILD_")
ENVIRONMENT_NAME = re.compile(r"[A-Z_][A-Z0-9_]{0,63}")
TARGET = re.compile(r"[A-Za-z_]\w*(?:\.[A-Za-z_]\w*)+")


def forced_exception(name: str) -> type | None:
    """A builtin exception a forcing may raise. AssertionError would be a failure by assertion
    the forcing itself wrote, and BaseException-only classes end the test process."""
    error = getattr(builtins, name, None) if name.isidentifier() else None
    if isinstance(error, type) and issubclass(error, Exception) and not issubclass(error, AssertionError):
        return error
    return None


@dataclass(frozen=True)
class Forcing:
    """A plan's declarative recreation of a flake's condition (#393). It is data: the
    harness's host process only bounds it; the test container applies it."""
    symptom: str
    environment: tuple[tuple[str, str], ...] = ()
    # (target, mode, argument as canonical JSON)
    patches: tuple[tuple[str, str, str], ...] = ()

    def __post_init__(self):
        if not isinstance(self.symptom, str) or not self.symptom.strip() or len(self.symptom) > FORCING_TEXT:
            raise HarnessInputError(f"a forcing needs a symptom of 1 to {FORCING_TEXT} characters")
        if not self.environment and not self.patches:
            raise HarnessInputError("a forcing needs at least one environment variable or patch")
        if len(self.environment) > FORCING_LIMIT or len(self.patches) > FORCING_LIMIT:
            raise HarnessInputError(f"a forcing has at most {FORCING_LIMIT} variables and {FORCING_LIMIT} patches")
        for name, value in self.environment:
            if (not isinstance(name, str) or not ENVIRONMENT_NAME.fullmatch(name) or name in RESERVED_ENVIRONMENT
                    or name.startswith(RESERVED_PREFIXES)):
                detail = (f"; {name} is a runner-owned native-thread bound set by the configured CPU demand"
                          if isinstance(name, str) and name in NATIVE_THREAD_ENVIRONMENT else "")
                raise HarnessInputError(f"a forcing can't set the environment variable {name!r}{detail}")
            if not isinstance(value, str) or len(value) > FORCING_TEXT or "\x00" in value:
                raise HarnessInputError(f"{name} needs a string value of at most {FORCING_TEXT} characters")
        for target, mode, argument in self.patches:
            if not isinstance(target, str) or len(target) > 200 or not TARGET.fullmatch(target):
                raise HarnessInputError(f"a patch target must be a dotted module attribute: {target!r}")
            if mode not in PATCH_MODES:
                raise HarnessInputError(f"a patch has exactly one of {', '.join(PATCH_MODES)}")
            if len(argument.encode()) > FORCING_LITERAL_BYTES:
                raise HarnessInputError(f"a patch literal is limited to {FORCING_LITERAL_BYTES} bytes of JSON")
            value = json.loads(argument)
            if mode == "raises" and not (isinstance(value, str) and forced_exception(value)):
                raise HarnessInputError(f"a forcing can raise only a builtin Exception other than AssertionError: "
                                        f"{value!r}")
            if mode == "delay_seconds" and (type(value) not in (int, float) or not 0 < value <= MAX_DELAY_SECONDS):
                raise HarnessInputError(f"delay_seconds must be above 0 and at most {MAX_DELAY_SECONDS}")

    @classmethod
    def parse(cls, data) -> "Forcing":
        if not isinstance(data, dict) or set(data) - {"symptom", "environment", "patches"}:
            extra = sorted(set(data) - {"symptom", "environment", "patches"}) if isinstance(data, dict) else []
            raise HarnessInputError(f"a forcing has only symptom, environment and patches; not {', '.join(extra)}")
        environment, patches = data.get("environment", {}), data.get("patches", [])
        if not isinstance(environment, dict) or not isinstance(patches, list):
            raise HarnessInputError("a forcing's environment is an object and its patches a list")
        parsed = []
        for patch in patches:
            if not isinstance(patch, dict) or "target" not in patch:
                raise HarnessInputError("each patch names a dotted target")
            modes = [mode for mode in patch if mode != "target"]
            if len(modes) != 1 or modes[0] not in PATCH_MODES:
                raise HarnessInputError(f"a patch has exactly one of {', '.join(PATCH_MODES)}")
            try:
                argument = json.dumps(patch[modes[0]], sort_keys=True, separators=(",", ":"), allow_nan=False)
            except (TypeError, ValueError) as error:
                raise HarnessInputError("a patch's argument must be a JSON literal") from error
            parsed.append((patch["target"], modes[0], argument))
        return cls(data.get("symptom"), tuple(sorted(environment.items())), tuple(parsed))

    def as_json(self) -> dict:
        return {"symptom": self.symptom, "environment": dict(self.environment),
                "patches": [{"target": target, mode: json.loads(argument)}
                            for target, mode, argument in self.patches]}


@dataclass(frozen=True)
class Check:
    name: str
    kind: Kind
    tests: tuple[str, ...]
    forcing: Forcing | None = None

    def __post_init__(self):
        if not self.name or not isinstance(self.kind, Kind) or not self.tests:
            raise HarnessInputError("a check needs a name, a kind and named tests")
        if self.forcing is not None and (self.kind != Kind.DEMONSTRATING or not isinstance(self.forcing, Forcing)):
            raise HarnessInputError("only a demonstrating check takes a forcing")
        if len(set(self.tests)) != len(self.tests):
            raise HarnessInputError("a check repeats a test")
        for test in self.tests:
            path, *names = test.split("::")
            if (not names and (self.kind != Kind.REGRESSION or self.name != ALWAYS_CHECK) or not all(names) or not path.endswith(".py")
                    or path.startswith(("/", "-")) or ".." in path.split("/")
                    or "\\" in path or "\x00" in test):
                raise HarnessInputError(f"not a pytest selector: {test!r}")


@dataclass(frozen=True)
class TestResult:
    test: str
    base: Outcome
    head: Outcome
    pre_existing: bool
    passed: bool
    message: str = ""
    # A forced check's head run under the forcing; `base` is then the base under it (#393).
    head_forced: Outcome | None = None


@dataclass(frozen=True)
class Result:
    name: str
    kind: Kind
    verdict: Verdict
    tests: tuple[TestResult, ...]
    custody: Custody = field(default=Custody.IN_PROCESS, init=False)
    forcing: Forcing | None = None


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


def forced_base_refusal(test: str, before: Outcome, notes: FailureNotes | None) -> str:
    """Why a forced test's base run did not reproduce the flake; empty when it did (#393)."""
    failure = (notes.details if notes is not None else {}).get(test)
    if failure is not None and failure.exception.startswith("ForcingError"):
        return (f"The plan's forcing could not be applied: {failure.exception}. The forcing is part of the "
                "plan, so no change to the code or tests can fix it; report the goal as not met and quote "
                "this message.")
    match before:
        case Outcome.ASSERTION:
            return ""
        case Outcome.PASSED:
            return ("This test passed on the base under the forcing, so the forcing does not reproduce the "
                    "flake. A forced reproduction needs the old test to fail by an assertion on the base while "
                    "the forcing holds.")
        case Outcome.MISSING | Outcome.UNCOLLECTED:
            return ("A forced reproduction runs the old test as the base wrote it, and this test is not "
                    "defined at the base. Name the existing flaky test.")
        case _:
            what = f"`{failure.exception}`" if failure is not None and failure.exception else str(before)
            return (f"On the base under the forcing this test ended with {what}, not an assertion failure, "
                    "so it does not reproduce the flake.")


def judge(check: Check, base: dict[str, Outcome], head: dict[str, Outcome], *,
          messages: dict[str, str] | None = None, base_notes: FailureNotes | None = None,
          forced: dict[str, Outcome] | None = None, forced_messages: dict[str, str] | None = None) -> Result:
    """`forced` holds a forced check's head outcomes under its forcing; `base` is then the
    original base under the forcing and `head` the head without it (#393)."""
    tests = []
    infrastructure = False
    for test in check.tests:
        before, after = base[test], head[test]
        during = forced[test] if check.forcing is not None else None
        infrastructure |= Outcome.NO_REPORT in (before, after, during)
        pre_existing = (check.kind == Kind.REGRESSION
                        and before in (Outcome.ASSERTION, Outcome.ERROR))
        match check.kind:
            case Kind.DEMONSTRATING if check.forcing is not None:
                passed = before == Outcome.ASSERTION and during == after == Outcome.PASSED
            case Kind.DEMONSTRATING | Kind.REPRODUCING:
                passed = before in (Outcome.ASSERTION, Outcome.ADDED_NAME) and after == Outcome.PASSED
            case Kind.REGRESSION:
                available = before not in (Outcome.SKIPPED, Outcome.MISSING, Outcome.UNCOLLECTED, Outcome.NO_REPORT)
                passed = available and (after == Outcome.PASSED or
                                         pre_existing and after in (Outcome.ASSERTION, Outcome.ERROR))
            case Kind.BEHAVIOR_PRESERVING:
                passed = before == after == Outcome.PASSED
        message = "" if passed else (messages or {}).get(test, "")
        if not passed and check.forcing is not None:
            if refusal := forced_base_refusal(test, before, base_notes):
                message = refusal
            elif during != Outcome.PASSED:
                message = (f"On the head under the forcing this test ended {during}: the fix must hold while "
                           "the flake's condition is forced.")
                if detail := (forced_messages or {}).get(test, ""):
                    message += f"\n{detail}"
            else:
                message = (f"On the head without the forcing this test ended {after}: the fix must not depend "
                           "on the forcing." + (f"\n{message}" if message else ""))
            if len(message) > MESSAGE_LIMIT:
                message = message[:MESSAGE_LIMIT - len(TRUNCATED)] + TRUNCATED
        elif not passed and check.kind in (Kind.DEMONSTRATING, Kind.REPRODUCING) and (
                refusal := base_refusal(check, test, before, base_notes)):
            message = refusal + (f"\nOn the head: {message}" if message else "")
            if len(message) > MESSAGE_LIMIT:
                message = message[:MESSAGE_LIMIT - len(TRUNCATED)] + TRUNCATED
        if not passed and not message:
            message = f"Check {check.kind}: base {before}, head {after}."
        tests.append(TestResult(test, before, after, pre_existing, passed, message[:MESSAGE_LIMIT], during))
    verdict = (Verdict.INFRASTRUCTURE_FAILURE if infrastructure else
               Verdict.PASS if all(test.passed for test in tests) else Verdict.FAIL)
    return Result(check.name, check.kind, verdict, tuple(tests), forcing=check.forcing)
