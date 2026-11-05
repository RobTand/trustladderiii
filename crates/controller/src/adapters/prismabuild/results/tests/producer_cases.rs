// Generated on BuildBuddy by producer_cases.py; verify with --check.
pub(super) const CASES: &str = r###"[
  {
    "custody": "in-process",
    "kind": "regression",
    "name": "regression",
    "tests": [
      {
        "base": "passed",
        "head": "no-report",
        "message": "Check regression: base passed, head no-report.",
        "passed": false,
        "phases": {
          "head": {
            "call": {
              "assertion": false,
              "exception": "",
              "outcome": "passed"
            },
            "setup": {
              "assertion": false,
              "exception": "",
              "outcome": "passed"
            },
            "teardown": {
              "assertion": false,
              "exception": "",
              "outcome": "passed"
            },
            "test": "tests/test_x.py::test_x"
          }
        },
        "pre_existing": false,
        "test": "tests/test_x.py::test_x"
      }
    ],
    "verdict": "infrastructure-failure"
  },
  {
    "custody": "in-process",
    "kind": "regression",
    "name": "Existing tests that import the change",
    "tests": [
      {
        "base": "passed",
        "head": "error",
        "message": "Complete execution group ('tests/test_x.py',); head collection unverified; required base collection 1 tests, sha256 dcb00132060ac9568e8e7413aeb8db2b13c90d2351d88f0f4e3ef141e769ee0e; retry unit: ('tests/test_x.py',).\nThe execution group ended its process on the head without a complete report; this complete group passed on the base.",
        "passed": false,
        "phases": {
          "head": {
            "call": {
              "assertion": false,
              "exception": "",
              "outcome": "passed"
            },
            "setup": {
              "assertion": false,
              "exception": "",
              "outcome": "passed"
            },
            "teardown": {
              "assertion": false,
              "exception": "",
              "outcome": "passed"
            },
            "test": "tests/test_x.py::test_x"
          }
        },
        "pre_existing": false,
        "test": "tests/test_x.py"
      }
    ],
    "verdict": "fail"
  }
]"###;
