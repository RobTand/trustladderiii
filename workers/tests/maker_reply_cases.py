"""Generate migration vectors with the unchanged pre-extraction implementation."""
from copy import deepcopy
import ast
import itertools
import json
from pathlib import Path
import sys

def reference():
    namespace = {"__name__": "tl3_workers.roles.maker_before"}
    exec(compile(Path(__file__).with_name("reference").joinpath("maker_before.py").read_text(),
                 "maker_before.py", "exec"), namespace)
    return namespace


def vectors():
    old = reference()
    rows = []

    def reply(name, inputs, value):
        try:
            old["_check_reply"](inputs, value)
            error = None
        except ValueError as failure:
            error = str(failure)
        rows.append({"operation": "reply", "name": name, "inputs": inputs, "reply": value, "error": error})

    inputs = {"goals": [{"id": "m"}, {"id": "c", "stage": "checking"},
                        {"id": "p", "stage": "publishing"}]}
    done = {"kind": "done", "self_check": [{"goal_id": "m", "status": "met"},
            {"goal_id": "c", "status": "pending"}, {"goal_id": "p", "status": "pending"}]}
    for ids in itertools.product(["m", "c", "p", "unknown"], repeat=3):
        value = deepcopy(done)
        for item, goal in zip(value["self_check"], ids):
            item["goal_id"] = goal
        reply("coverage/" + "/".join(ids), inputs, value)
    for statuses in itertools.product(["met", "pending", "not_met"], repeat=3):
        value = deepcopy(done)
        for item, status in zip(value["self_check"], statuses):
            item["status"] = status
        reply("stages/" + "/".join(statuses), inputs, value)
    proof = {"subjects": [], "observer_support": [], "red_message": "regression"}
    base = {"goals": [], "current_review_findings": [{"id": "f"}]}
    for ids in [[], ["f"], ["other"], ["f", "f"]]:
        reply("reproductions/" + repr(ids), base, {"kind": "done", "self_check": [],
              "reproductions": [{"finding_id": name, "reason": "not executable"} for name in ids]})
    declarations = [None, {}, {**proof, "extra": 1}, {**proof, "subjects": None},
                    {**proof, "subjects": ["a.py"] * 17}, {**proof, "subjects": ["../a.py"]},
                    {**proof, "subjects": ["a.py", "a.py"]}, {**proof, "red_message": ""},
                    {**proof, "red_message": "r" * 161}, proof]
    for index, declaration in enumerate(declarations):
        value = {"kind": "done", "self_check": [], "reproductions": [{"finding_id": "f",
                 "tests": ["test_a.py::test_a"], "proof": declaration}]}
        reply(f"proof/{index}", base, value)
    for tests in [[], ["a.py::test_a"] * 2, ["a.py"], ["../a.py::test_a"],
                  ["a.py::"], ["a.py::test_a[é'\\n]"], ["a.py::test_a"]]:
        reply("selectors/" + repr(tests), base, {"kind": "done", "self_check": [],
              "reproductions": [{"finding_id": "f", "tests": tests, "proof": proof}]})
    for path in ["", "/a", "a\\b", "a/../b", ".git/config", ".", "./", "a//b", "a/" + "é" * 128]:
        reply("path/" + path, inputs, {"kind": "needs_files", "files": [{"path": path}]})
    # Literal reply dictionaries from every maker module, including incident tests.
    for source in sorted(Path(__file__).parent.glob("test_maker*.py")):
        for node in ast.walk(ast.parse(source.read_text())):
            if not isinstance(node, ast.Dict):
                continue
            try:
                value = ast.literal_eval(node)
            except (ValueError, TypeError):
                continue
            if not isinstance(value, dict) or value.get("kind") not in {"done", "needs_files", "cant_meet", "blocked", "out_of_room"}:
                continue
            goals = [{"id": item["goal_id"]} for item in value.get("self_check", [])]
            try:
                reply(f"{source.name}:{node.lineno}", {"goals": goals}, value)
            except (KeyError, TypeError):
                continue
    checks = [{"name": "ordinary", "tests": ["a.py::test_a"], "kind": "demonstrating"},
              {"name": "regression", "tests": ["a.py::test_b"], "kind": "regression"},
              {"name": "forced", "tests": ["a.py::test_c"], "kind": "demonstrating", "forcing": {}},
              {"name": "symptom", "tests": ["a.py::test_d"], "kind": "demonstrating", "declared_symptom": {}}]
    for base_sha in [None, "", "base"]:
        for count in range(5):
            data = {"check_base_sha": base_sha, "plan": {"checks": checks[:count]}}
            rows.append({"operation": "handoff", "name": f"handoff/{base_sha}/{count}", "inputs": data,
                         "handoff": old["_checking_handoff"](data, "head", "tree")})
    return rows


if __name__ == "__main__":
    json.dump(vectors(), sys.stdout, ensure_ascii=False)
