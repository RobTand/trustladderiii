// Review counterexample derived from the committed declared-symptom vector.
pub(super) const CASE: &str = r###"{
  "fixture": "review counterexample: existing declared-symptom vector, valid planned proof added, ordinary head changed to error",
  "request": {
    "base": "f7edff4c7a62c7925cfb86f5ef6904df7ffc09f2",
    "effect_key": "checks:declared-symptom",
    "head": "d3d2ec2fef24fa8a984b56ec991a1f5737169be6",
    "image": "sha256:1111111111111111111111111111111111111111111111111111111111111111",
    "plan": [
      {
        "declared_symptom": {
          "exception": "builtins.TimeoutError",
          "node": "tests/test_original.py::test_original",
          "parallel": 0,
          "phase": "call",
          "repeat": 1
        },
        "forcing": {
          "environment": {
            "REPRODUCE": "1"
          },
          "patches": [],
          "symptom": "The original call times out."
        },
        "kind": "demonstrating",
        "name": "original symptom",
        "tests": [
          "tests/test_original.py::test_original"
        ],
        "proof": {
          "subjects": [
            "tests/test_original.py"
          ],
          "observer_support": [],
          "red_message": "deadline passed"
        }
      }
    ],
    "reviewed": null
  },
  "results": [
    {
      "custody": "in-process",
      "declared_symptom": {
        "exception": "builtins.TimeoutError",
        "node": "tests/test_original.py::test_original",
        "parallel": 0,
        "phase": "call",
        "repeat": 1
      },
      "forcing": {
        "environment": {
          "REPRODUCE": "1"
        },
        "patches": [],
        "symptom": "The original call times out."
      },
      "kind": "demonstrating",
      "name": "original symptom",
      "proof": {
        "base_tree_sha256": "a760ed930d11d30420812de23e57187eb8158753ef713baf4b2dc8855e47969f",
        "declaration": {
          "subjects": [
            "tests/test_original.py"
          ],
          "observer_support": [],
          "red_message": "deadline passed"
        },
        "head_tree_sha256": "aa44cfec0c74fdcc1cca9b7368dce491f0ed0869a99b45de07b7778e3cc9ccad",
        "mode": "head-observer",
        "observer_sha256": {
          "tests/test_original.py::test_original": "f7c48a9ae14783fe59d247d05db418e385cd91de9b6b1cad2114b1684ed9e52f"
        },
        "rule": "subject-observer-v1",
        "source_base_tree_sha256": "a760ed930d11d30420812de23e57187eb8158753ef713baf4b2dc8855e47969f",
        "subjects": {
          "tests/test_original.py": {
            "base": "e47c86f16177626a8e6a6c953bdf5e6ffc7872b8844fa5bdc938c95c4cdc33c2",
            "head": "f7c48a9ae14783fe59d247d05db418e385cd91de9b6b1cad2114b1684ed9e52f"
          }
        },
        "support_sha256": {}
      },
      "symptom_evidence": {
        "base": "f7edff4c7a62c7925cfb86f5ef6904df7ffc09f2",
        "declaration": {
          "exception": "builtins.TimeoutError",
          "node": "tests/test_original.py::test_original",
          "parallel": 0,
          "phase": "call",
          "repeat": 1
        },
        "forcing": {
          "environment": {
            "REPRODUCE": "1"
          },
          "patches": [],
          "symptom": "The original call times out."
        },
        "head": "d3d2ec2fef24fa8a984b56ec991a1f5737169be6",
        "runs": [
          {
            "outcome": "symptom",
            "records": [
              {
                "event": "start",
                "schema": 1
              },
              {
                "event": "result",
                "failures": [
                  {
                    "exception": "builtins.TimeoutError",
                    "phase": "call",
                    "source": true
                  }
                ],
                "message": "def test_original():\n        if os.getenv(\"REPRODUCE\"):\n>           raise TimeoutError(\"deadline passed\")\nE           TimeoutError: deadline passed\n\ntests/test_original.py:5: TimeoutError",
                "outcome": "error",
                "test": "tests/test_original.py::test_original",
                "truncated": false
              },
              {
                "event": "finish",
                "exit_code": 1
              }
            ],
            "run": 1,
            "sha256": "a553df0807843fbd0cfa64a641343edd7e99c7e9690fcf2fea19ae1784b0ea5b",
            "side": "base",
            "target_hits": {}
          },
          {
            "outcome": "passed",
            "records": [
              {
                "event": "start",
                "schema": 1
              },
              {
                "event": "result",
                "failures": [],
                "message": "",
                "outcome": "passed",
                "test": "tests/test_original.py::test_original",
                "truncated": false
              },
              {
                "event": "finish",
                "exit_code": 0
              }
            ],
            "run": 1,
            "sha256": "aea81c64825044409182b86ad33c7054fd3d2b02f04fab4b4f947277780da2a8",
            "side": "head",
            "target_hits": {}
          },
          {
            "outcome": "passed",
            "records": [
              {
                "event": "start",
                "schema": 1
              },
              {
                "event": "result",
                "failures": [],
                "message": "",
                "outcome": "passed",
                "test": "tests/test_original.py::test_original",
                "truncated": false
              },
              {
                "event": "finish",
                "exit_code": 0
              }
            ],
            "run": 1,
            "sha256": "aea81c64825044409182b86ad33c7054fd3d2b02f04fab4b4f947277780da2a8",
            "side": "ordinary-head",
            "target_hits": {}
          }
        ]
      },
      "tests": [
        {
          "base": "error",
          "head": "error",
          "head_forced": "passed",
          "message": "",
          "passed": true,
          "pre_existing": false,
          "test": "tests/test_original.py::test_original"
        }
      ],
      "verdict": "pass"
    }
  ]
}
"###;
