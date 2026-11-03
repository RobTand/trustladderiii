"""Report plan measurements and rule violations to the controller."""

from copy import deepcopy
import configparser
import ast
import tomllib
from pathlib import PurePosixPath
import re

from jsonschema import Draft202012Validator

from tl3_workers.roles.base import Role
from tl3_workers.evidence import preflight
from tl3_workers.forecast import Forecaster, HARD_CAP_BYTES, MAX_EXECUTION_PIECES, execution_forecast
from tl3_harness import Check, Forcing, HarnessInputError, Kind, Proof
from tl3_harness.proof import overlaps
from tl3_harness.limits import validate_check_plan
from tl3_harness.node_ids import validate_always_tests
from tl3_harness.trees import is_test, test_function_source
from tl3_workers.tools import Worktree
from tl3_workers.generated_scope import configured_outputs
from tl3_workers.recall import check_citations
from tl3_workers.turns import REPAIR_MESSAGE_BYTES


# Fresh proposals require estimates; the transport still accepts workers that
# started before a deploy (#372). Use the same requirement in the model prompt.
EXPLORATION_DECLARATION = {
    "if": {"properties": {"kind": {"enum": ["leaf", "split"]}}, "required": ["kind"]},
    "then": {"required": ["estimated_exploration_turns"],
             "properties": {"pieces": {"items": {"required": ["estimated_exploration_turns"]}}}},
}


def execution_budget_details(execution):
    components = ", ".join(f"{name}={value}" for name, value in execution["components"].items())
    return (f"estimated {execution['estimated_tool_turns']} tool turns, "
            f"usable {execution['usable_tool_turns']} after the finalization reserve. "
            f"Components: {components}.")


class PlanConsistencyError(ValueError):
    """A draft's checks disagree with its declarations or exact base selections."""

    def __init__(self, uncovered, missing_forced=(), *, draft=None, file_selections=None):
        self.draft = deepcopy(draft)
        self.uncovered = set(uncovered)
        self.missing_forced = set(missing_forced)
        self.file_selections = file_selections or {}
        self.node_ids = (*uncovered, *missing_forced, *self.file_selections)
        self.violations = [
            f"`{node_id}` is in edits_tests but no demonstrating check names it. "
            "Name it in a demonstrating check that fails on the base by an assertion."
            for node_id in uncovered]
        self.violations.extend(f"Existing check must name a test defined at the base: {node_id}"
                               for node_id in missing_forced)
        self.violations.extend(f"Checks must name exact pytest node IDs: {path}. "
                               f"Replace the file with every already-read base definition: {', '.join(nodes)}"
                               for path, nodes in self.file_selections.items())
        super().__init__("\n".join(self.violations) +
                         "\nReconcile the declaration and checks using already-read facts. "
                         "Keep the draft's files, goals, check kinds and forcings. Only add missing "
                         "demonstrating coverage, replace forced non-base selections with base tests, "
                         "replace regression file selections with their complete base definitions, "
                         "or drop an uncovered unnecessary edit. Do not expand the draft. "
                         "Drop an edit only if it is unnecessary for the goals. "
                         "A forced check must name existing base tests and preserve every required goal, "
                         "including deterministic and mutant proof; do not treat a passing smoke as red. "
                         "Do not weaken goals or invent base failures; return cant_meet if needed.")

    def constrain(self, reply):
        """Keep the correction within the declarations of the rejected draft."""
        draft = self.draft
        errors = []
        if reply['kind'] not in {'leaf', 'cant_meet'}:
            reply['violations'].append('Correction must remain a leaf or honestly report cant_meet.')
            return
        added = set(reply['files']) - set(draft['files'])
        if added:
            errors.append('Correction expands the draft footprint: ' + ', '.join(sorted(added)))
        if reply['kind'] == 'cant_meet':
            reply['violations'].extend(errors)
            return
        if set(reply['files']) != set(draft['files']) and not added:
            errors.append('Correction changes the draft footprint.')
        mutable = {'checks', 'edits_tests', 'forecast', 'execution', 'violations', 'decision_context', 'evidence_errors', 'forcing_rehearsals'}
        if ({key: value for key, value in reply.items() if key not in mutable}
                != {key: value for key, value in draft.items() if key not in mutable}):
            errors.append('Correction changes declarations outside the test consistency error.')
        before, after = set(draft.get('edits_tests', [])), set(reply.get('edits_tests', []))
        if after - before or (before - after) - self.uncovered:
            errors.append('Correction may only drop uncovered edit declarations.')
        checks = draft['checks']
        if len(reply['checks']) < len(checks):
            errors.append('Correction removes a draft check.')
        uncovered = {function(node) for node in self.uncovered}
        goals = {goal for check in checks for goal in check['goal_ids']}
        for index, check in enumerate(reply['checks']):
            if not set(check['goal_ids']) <= goals:
                errors.append('Correction introduces a goal outside the draft checks.')
            if index >= len(checks):
                if (check['kind'] != 'demonstrating'
                        or any(function(node) not in uncovered for node in check['node_ids'])):
                    errors.append('Correction may only add demonstrating checks for uncovered edits.')
                if ('forcing' in check
                        and not any(check['forcing'] == original.get('forcing') for original in checks)):
                    errors.append('Correction adds a forcing absent from the draft.')
                if ('declared_symptom' in check and not any(
                        check['declared_symptom'] == original.get('declared_symptom') for original in checks)):
                    errors.append('Correction adds a declared symptom absent from the draft.')
                continue
            original = checks[index]
            if ({key: value for key, value in check.items() if key != 'node_ids'}
                    != {key: value for key, value in original.items() if key != 'node_ids'}):
                errors.append('Correction changes a draft check kind, goals or forcing.')
            old_nodes, new_nodes = set(original['node_ids']), set(check['node_ids'])
            expanded = old_nodes & self.file_selections.keys()
            if expanded:
                expected = old_nodes - expanded
                expected.update(node for path in expanded for node in self.file_selections[path])
                if new_nodes != expected:
                    errors.append('Correction must preserve the complete file selection without extra tests.')
                continue
            replacing = original['kind'] == 'demonstrating' and 'forcing' in original
            removable = self.missing_forced if replacing else set()
            if (old_nodes - new_nodes) - removable:
                errors.append('Correction removes a required draft test selection.')
            if not (replacing and old_nodes & self.missing_forced):
                if any(check['kind'] != 'demonstrating' or function(node) not in uncovered
                       for node in new_nodes - old_nodes):
                    errors.append('Correction adds test selections outside the draft consistency error.')
        reply['violations'].extend(dict.fromkeys(errors))


def check_reply(inputs, reply, *, rehearsals=None, read_sources=None):
    if rehearsals is not None:
        rehearsals.raise_infrastructure()
    Draft202012Validator(EXPLORATION_DECLARATION).validate(reply)
    if any(field in reply for field in ("forecast", "violations", "decision_context", "forcing_rehearsals")):
        raise ValueError("only code may supply planning evidence")
    preflight(inputs, reply, planning=True)
    check_citations(inputs, reply)
    goals = set(range(len(inputs["ingest"]["goals"])))
    violations = (["Execution forecasts are supplied by worker code; omit execution from the model reply."]
                  if reply.pop("execution", None) is not None else [])
    uncovered = []
    missing_forced = []
    file_selections = {}
    invalid_selectors = False
    paths = set()
    if reply["kind"] == "leaf":
        if inputs["must_split"]:
            violations.append("Must split after running out of room.")
        if not reply["files"]:
            violations.append("A leaf must declare its files.")
        if not reply["no_design_questions"]:
            violations.append("A leaf must resolve every design question.")
        if not reply["lands_on_its_own"]:
            violations.append("A leaf must land on its own.")
        for check in reply["checks"]:
            if check['kind'] in ('demonstrating', 'reproducing') and 'declared_symptom' not in check and 'proof' not in check:
                violations.append('Assertion RED requires proof.red_message; use subjects: [] for an inferred partition.')
            if 'proof' in check:
                try:
                    proof = Proof.parse(check['proof'])
                    for unit in (*proof.subjects, *proof.observer_support):
                        if unit.split('::')[0] not in reply['files']:
                            violations.append(f'Proof source must be in declared files: {unit}')
                except HarnessInputError as error:
                    violations.append(str(error))
        covered = set()
        forcings = {}
        for index, check in enumerate(reply["checks"]):
            if "forcing" in check:
                # #393: a forced reproduction is the red for a deflake.
                if check["kind"] != "demonstrating":
                    violations.append("Only a demonstrating check can carry a forcing.")
                try:
                    forcings[index] = Forcing.parse(check["forcing"])
                except HarnessInputError as error:
                    violations.append(f"Invalid forcing: {error}")
            if not check["node_ids"]:
                violations.append("Every check must name at least one test.")
            if not check["goal_ids"]:
                violations.append("Every check must cite at least one goal.")
            for node_id in check["node_ids"]:
                try:
                    Check("planned", Kind(check["kind"]), (node_id,))
                except ValueError:
                    invalid_selectors = True
                    if check['kind'] == 'regression' and '::' not in node_id:
                        nodes, problem = base_file_selection(node_id, read_sources or {})
                        if problem:
                            violations.append(f"Checks must name exact pytest node IDs: {node_id}. {problem}")
                        else:
                            file_selections[node_id] = nodes
                    else:
                        violations.append("Checks must name exact pytest node IDs.")
                    continue
                # A forced check runs the old test on the base, so it must exist there too.
                if check["kind"] in {"regression", "behavior-preserving"} or ("forcing" in check and not check.get("proof", {}).get("subjects")) or "declared_symptom" in check:
                    path = node_id.split("::")[0]
                    oversized, exists = base_definition(node_id)
                    if oversized:
                        violations.append(f"Existing check's test file is over the {HARD_CAP_BYTES}-byte limit: {path}")
                    elif not exists:
                        if check["kind"] == "demonstrating" and "forcing" in check:
                            missing_forced.append(node_id)
                        else:
                            violations.append(f"Existing check must name a test defined at the base: {node_id}")
            cited = set(check["goal_ids"])
            if not cited <= goals:
                violations.append("A check cites an unknown goal.")
            covered.update(cited)
        edit_errors, uncovered = edit_violations(inputs["ingest"]["goals"], reply)
        violations.extend(edit_errors)
        if not invalid_selectors or file_selections:
            try:
                validate_check_plan(tuple(Check(f'check-{i + 1}', Kind(check['kind']),
                                                tuple(node for item in check['node_ids']
                                                      for node in file_selections.get(item, (item,))),
                                                (forcings.get(i) if check['kind'] == 'demonstrating' else None),
                                                Proof.parse(check['proof']) if 'proof' in check else None,
                                                check.get('declared_symptom'))
                                          for i, check in enumerate(reply['checks']) if check['node_ids']))
            except ValueError as error:
                violations.append(str(error))
        required = {i for i, goal in enumerate(inputs["ingest"]["goals"])
                    if goal.get("stage", "maker") != "publishing"}
        if not required <= covered:
            violations.append("Every maker and checking goal must have a check.")
        paths.update(reply["files"])
    elif reply["kind"] == "split":
        covered = set()
        for piece in reply["pieces"]:
            if not piece["goal_ids"]:
                violations.append("Every piece must cite at least one goal.")
            cited = set(piece["goal_ids"])
            if not cited <= goals:
                violations.append("A piece cites an unknown goal.")
            covered.update(cited)
            paths.update(piece["files"])
        if len(reply["pieces"]) < 2:
            violations.append("A split must propose at least two pieces.")
        if covered != goals:
            violations.append("A split must preserve every goal.")
    else:
        paths.update(reply["files"])

    if reply["kind"] in {"leaf", "split"}:
        for path in sorted(paths):
            if any(PurePosixPath(path).full_match(pattern) for pattern in inputs["protected_paths"]):
                violations.append(f"Declared file matches a protected path: {path}")

    if reply["kind"] in {"leaf", "split"} and inputs["forecast"].get("generator"):
        with Worktree(".") as tree:
            outputs = configured_outputs(tree, inputs["forecast"], inputs["protected_paths"])
        paths.update(outputs)
        if reply["kind"] == "leaf":
            reply["files"] = sorted(paths)
        else:
            for piece in reply["pieces"]:
                piece["files"] = sorted(set(piece["files"]) | set(outputs))

    forecaster = Forecaster(inputs["forecast"])
    forecast = forecaster.measure(inputs["ingest"]["brief"], paths)
    check_count = (len(reply["checks"]) if reply["kind"] == "leaf" else len(goals))
    execution = execution_forecast(paths, check_count, declared=reply.get("estimated_tool_turns", 0),
                                   exploration=reply.get("estimated_exploration_turns"),
                                   config=inputs["forecast"])
    if reply["kind"] == "leaf" and execution["exploration_declared"] and not execution["fits"]:
        violations.append(f"The leaf exceeds the execution budget: {execution_budget_details(execution)} "
                          "Preserve every obligation in bounded pieces with reason execution_budget before maker work.")
    # A file the tools can't use is a plan problem, not a crash (#321).
    for error in forecaster.problems:
        violations.append(f"Declared file can't be used: {error}. Leave it out.")
    if reply["kind"] == "leaf" and not forecast["fits"]:
        violations.append("The leaf forecast does not fit. Propose smaller pieces.")
    if reply["kind"] == "split":
        reason = reply["reason"]
        if reason == "multiple_repos":
            violations.append("This request is in one repository.")
        if reason == "out_of_room" and not inputs["must_split"]:
            violations.append("No maker has run out of room.")
        if reason == "does_not_fit" and forecast["fits"]:
            violations.append("The forecast fits. Splitting needs a permitted reason.")
        if reason == "execution_budget":
            if execution["fits"]:
                violations.append("The execution estimate fits; execution_budget is not a permitted reason for this plan.")
            if len(reply["pieces"]) > MAX_EXECUTION_PIECES:
                violations.append(f"Execution-budget decomposition permits at most {MAX_EXECUTION_PIECES} pieces.")
        pieces = []
        for piece in reply["pieces"]:
            child = forecaster.measure(piece["brief"], piece["files"])
            estimate = execution_forecast(piece["files"], len(piece["goal_ids"]),
                                          declared=piece.get("estimated_tool_turns", 0),
                                          exploration=piece.get("estimated_exploration_turns"),
                                          config=inputs["forecast"])
            pieces.append(estimate)
            for error in forecaster.problems:
                violations.append(f"Declared file can't be used: {error}. Leave it out.")
            if reason == "execution_budget":
                if not estimate["fits"] or estimate["estimated_tool_turns"] >= execution["estimated_tool_turns"]:
                    violations.append("Every execution-budget piece must fit the usable execution allowance and reduce demand. "
                                      + execution_budget_details(estimate))
                if not child["fits"]:
                    violations.append("Every execution-budget piece must also fit the token forecast.")
            elif child["tokens"] >= forecast["tokens"]:
                violations.append("Every proposed piece must have a smaller forecast than its parent.")
        execution["pieces"] = pieces
        execution["aggregate_tool_turns"] = sum(piece["estimated_tool_turns"] for piece in pieces)
        execution["aggregate_allowed_tool_turns"] = sum(piece["allowed_tool_turns"] for piece in pieces)
    if rehearsals is not None and rehearsals.failure:
        violations.append(rehearsals.failure['message'])
    elif rehearsals is not None and reply['kind'] == 'cant_meet':
        rejected = next((item for item in reversed(rehearsals.candidates) if item['status'] == 'refused'), None)
        if rejected is not None:
            violations.append(rejected['message'])
    if uncovered or missing_forced or file_selections:
        mismatch = PlanConsistencyError(uncovered, missing_forced, draft=reply, file_selections=file_selections)
        # Only these intrinsic inconsistencies enter the existing tool-free repair.
        # Substantive plan failures still reach the controller under Q54.
        proof_errors = (rehearsals.violations(inputs.get('base_sha'),
                        [check for check in reply['checks'] if valid_forcing(check)])
                        if file_selections and rehearsals is not None else [])
        # File expansion must also fit the existing repair prompt; never send a partial list.
        message_fits = not file_selections or len(str(mismatch).encode()) <= REPAIR_MESSAGE_BYTES
        if not message_fits:
            violations.append('Whole-file selection exceeds the existing correction message limit.')
        if not violations and not proof_errors and inputs.get('repair_plan_consistency', False):
            raise mismatch
        violations.extend(mismatch.violations)
    if reply['kind'] == 'leaf':
        valid = [check for check in reply['checks'] if valid_forcing(check)]
        if rehearsals is not None:
            violations.extend(rehearsals.violations(inputs.get('base_sha'), valid))
        elif valid:
            violations.append('Q55 check lacks a matching forcing rehearsal receipt for the exact base, selectors and forcing.')
    if rehearsals is not None and (rehearsals.candidates or rehearsals.failure):
        reply['forcing_rehearsals'] = deepcopy(rehearsals.candidates + ([rehearsals.failure] if rehearsals.failure else []))
    if "decision_context" in inputs:
        reply["decision_context"] = inputs["decision_context"]
    reply["forecast"] = forecast
    reply["execution"] = execution
    reply["violations"] = list(dict.fromkeys(violations))


def test_files():
    """Configured base test patterns, read without importing served code."""
    try:
        with Worktree('.') as tree, tree._file('pyproject.toml') as stream:
            patterns = tomllib.loads(stream.read(HARD_CAP_BYTES))['tool']['tl3']['harness']['test-files']
    except (OSError, ValueError, UnicodeError, KeyError, TypeError):
        return None
    return patterns if isinstance(patterns, list) and all(isinstance(item, str) for item in patterns) else None


def discovery_text(tree, path):
    """Read optional base metadata without following links or echoing its contents."""
    try:
        with tree._file(path) as stream:
            source = stream.read(HARD_CAP_BYTES + 1)
    except FileNotFoundError:
        return None, None
    except ValueError as error:
        return (None, None) if str(error) == 'File not found' else (None, 'unavailable discovery metadata')
    except (OSError, UnicodeError):
        return None, 'unavailable discovery metadata'
    if len(source.encode()) > HARD_CAP_BYTES:
        return None, 'discovery metadata exceeds the byte limit'
    return source, None


COLLECTION_DEFAULTS = {
    'python_files': ('test_*.py', '*_test.py'),
    'python_functions': ('test',),
    'python_classes': ('Test',),
}


def collection_settings(name, source):
    """Accept only certain default collection, independently of config precedence."""
    try:
        if name == 'pyproject.toml':
            settings = tomllib.loads(source).get('tool', {}).get('pytest', {}).get('ini_options', {})
        else:
            config = configparser.ConfigParser(interpolation=None)
            config.read_string(source)
            if config.defaults() or name == 'setup.cfg' and config.has_section('pytest'):
                return 'uncertain collection settings'
            section = 'tool:pytest' if name == 'setup.cfg' else 'pytest'
            settings = dict(config[section]) if config.has_section(section) else {}
        if not isinstance(settings, dict):
            return 'uncertain collection settings'
        for key, default in COLLECTION_DEFAULTS.items():
            value = settings.get(key, default)
            if isinstance(value, str):
                value = value.split()
            if not isinstance(value, (list, tuple)) or tuple(value) != default:
                return 'non-default collection settings'
        # Explicit plugin loading can replace collection even with default prefixes.
        options = settings.get('addopts', '')
        if not isinstance(options, (str, list, tuple)):
            return 'uncertain collection settings'
        words = options.split() if isinstance(options, str) else options
        if (not all(isinstance(word, str) for word in words)
                or any(word.startswith(('-p', '--override-ini', '-o')) for word in words)
                or settings.get('required_plugins')):
            return 'dynamic plugin collection settings'
    except (ValueError, TypeError, AttributeError, configparser.Error):
        return 'unavailable collection settings'
    return None


def collection_context(path):
    """Inspect all ancestor config/conftest files; uncertain precedence refuses too."""
    parents = list(reversed(PurePosixPath(path).parents))
    if len(parents) > 64:
        return 'discovery ancestor limit'
    used = 0
    with Worktree('.') as tree:
        for parent in parents:
            for name in ('pytest.ini', '.pytest.ini', 'pyproject.toml', 'setup.cfg', 'tox.ini',
                         'pytest.toml', '.pytest.toml', 'conftest.py'):
                source, problem = discovery_text(tree, str(parent / name))
                if problem:
                    return 'unavailable collection settings' if name != 'conftest.py' else problem
                if source is None:
                    continue
                used += len(source.encode())
                if used > HARD_CAP_BYTES:
                    return 'discovery metadata exceeds the byte limit'
                if name == 'conftest.py':
                    _, problem = static_test_definitions(source, str(parent / name), collect=False)
                elif name.endswith('.toml') and name != 'pyproject.toml':
                    problem = 'unsupported collection settings'
                else:
                    problem = collection_settings(name, source)
                if problem:
                    return problem
    return None


def literal_or_name(node):
    if isinstance(node, ast.Name):
        return True
    try:
        ast.literal_eval(node)
        return True
    except (ValueError, TypeError):
        return False


def static_test_definitions(source, path, *, collect=True):
    """Allow static statements and inert definition expressions, never execute Python."""
    nodes = []
    selection_bytes = 0

    def scan(body, prefix, collecting):
        nonlocal selection_bytes
        seen = set()
        for node in body:
            if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
                names = [node.name]
                if node.name.startswith('pytest_') or node.name in {'__getattr__', '__init__', '__new__'}:
                    return 'dynamic collection hooks'
                if getattr(node, 'type_params', ()):
                    return 'dynamic definition expressions'
                for decorator in node.decorator_list:
                    mark = decorator.func if isinstance(decorator, ast.Call) else decorator
                    if not (isinstance(mark, ast.Attribute) and isinstance(mark.value, ast.Attribute)
                            and mark.value.attr == 'mark' and isinstance(mark.value.value, ast.Name)
                            and mark.value.value.id == 'pytest'):
                        return 'dynamic decorators'
                    if isinstance(decorator, ast.Call) and (
                            not all(literal_or_name(arg) for arg in decorator.args)
                            or not all(item.arg is not None and literal_or_name(item.value)
                                       for item in decorator.keywords)):
                        return 'dynamic decorator arguments'
                if isinstance(node, ast.ClassDef):
                    if prefix != path or node.bases or node.keywords:
                        return 'dynamic class discovery'
                    problem = scan(node.body, prefix + '::' + node.name,
                                   collecting and node.name.startswith('Test'))
                    if problem:
                        return problem
                else:
                    annotations = [arg.annotation for arg in (*node.args.posonlyargs, *node.args.args,
                                   *node.args.kwonlyargs) if arg.annotation is not None]
                    annotations.extend(arg.annotation for arg in (node.args.vararg, node.args.kwarg)
                                       if arg is not None and arg.annotation is not None)
                    values = [*node.args.defaults, *(value for value in node.args.kw_defaults if value is not None),
                              *annotations, *([node.returns] if node.returns is not None else [])]
                    if not all(literal_or_name(value) for value in values):
                        return 'dynamic definition expressions'
                    # Pytest's default is a prefix, not the narrower test_ convention.
                    if collecting and node.name.startswith('test'):
                        selector = prefix + '::' + node.name
                        selection_bytes += len(selector.encode()) + 2
                        if selection_bytes > REPAIR_MESSAGE_BYTES // 2:
                            return 'base discovery exceeds the bounded correction message limit'
                        Check('planned', Kind.REGRESSION, (selector,))
                        nodes.append(selector)
            elif isinstance(node, (ast.Import, ast.ImportFrom)):
                names = [alias.asname or alias.name.split('.')[0] for alias in node.names]
                if (any(name == '*' or name.startswith(('test', 'Test', 'pytest_')) for name in names)
                        or any(alias.name.startswith('pytest_') for alias in node.names)):
                    return 'dynamic imported tests or collection hooks'
                if 'pytest' in names and not (isinstance(node, ast.Import)
                        and any(alias.name == 'pytest' and alias.asname in (None, 'pytest') for alias in node.names)):
                    return 'dynamic pytest binding'
            elif isinstance(node, (ast.Assign, ast.AnnAssign)):
                targets = node.targets if isinstance(node, ast.Assign) else [node.target]
                if not all(isinstance(target, ast.Name) for target in targets):
                    return 'dynamic bindings'
                names = [target.id for target in targets]
                if any(name.startswith(('test', 'Test', 'pytest')) or name == '__test__' for name in names):
                    return 'dynamic test bindings'
                if (not literal_or_name(node.value)
                        or isinstance(node, ast.AnnAssign) and not literal_or_name(node.annotation)
                        or '__all__' in names and isinstance(node.value, ast.Name)):
                    return 'dynamic module initialization'
            elif isinstance(node, ast.Pass) or (isinstance(node, ast.Expr)
                    and isinstance(node.value, ast.Constant) and isinstance(node.value.value, str)):
                names = []
            else:
                return 'dynamic module discovery'
            if seen.intersection(names) or len(set(names)) != len(names):
                return 'ambiguous definitions'
            seen.update(names)
        return None

    try:
        problem = scan(ast.parse(source).body, path, collect)
    except (SyntaxError, ValueError, RecursionError, MemoryError):
        problem = 'unavailable definitions'
    return tuple(nodes), problem


def base_file_selection(path, read_sources):
    """Enumerate only unambiguous static definitions already visible in the same run."""
    try:
        validate_always_tests([path])
        patterns = test_files()
        if patterns is None or not is_test(path, patterns):
            return (), 'unavailable configured test definitions'
        with Worktree('.') as tree:
            source, problem = discovery_text(tree, path)
        if problem:
            return (), problem
        if source is None:
            return (), 'unavailable base definitions'
        nodes, problem = static_test_definitions(source, path)
        if problem:
            return (), problem
        problem = collection_context(path)
        if problem:
            return (), problem
    except (OSError, ValueError, UnicodeError, RecursionError, MemoryError):
        return (), 'unavailable base definitions'
    if not nodes:
        return (), 'unavailable base definitions'
    if read_sources.get(path) != source:
        return (), 'unavailable complete already-read base definitions'
    return nodes, None


def base_definition(node_id):
    """Whether the base defines the test, after whether its file is over the hard cap."""
    # The whole module is parsed to find the test, so the
    # declared-file hard cap bounds it, not a page size.
    try:
        with Worktree(".") as tree, tree._file(node_id.split("::")[0]) as stream:
            source = stream.read(HARD_CAP_BYTES + 1)
    except (OSError, ValueError, UnicodeError):
        return False, False
    if len(source.encode("utf-8")) > HARD_CAP_BYTES:
        return True, False
    return False, test_function_source(source, node_id) is not None


EDIT_VERBS = frozenset(("extend", "extends", "extending", "patch", "patches", "patching", "update", "updates",
                        "updating", "modify", "modifies", "modifying", "rewrite", "rewrites", "rewriting",
                        "change", "changes", "changing", "edit", "edits", "editing", "fix", "fixes", "fixing",
                        "replace", "replaces", "replacing"))
ADD_VERBS = frozenset(("add", "adds", "adding"))
HOLDS = frozenset(("not", "never", "don't", "don’t", "dont", "nothing", "without", "unchanged", "keep",
                   "keeps", "keeping", "leave", "leaves", "leaving", "preserve", "preserves", "preserving"))
CLAUSES = frozenset(("so", "that", "until", "while", "and", "because", "when", "then", "but", "or", "if"))
WORD = re.compile(r"[a-z][\w'’-]*")


def mentions(text, test):
    """Each mention of a test's name, as the words before and after it in its sentence.
    A path such as `tests/<name>.py` isn't a mention."""
    for sentence in re.split(r"[.;!?](?:\s|$)|\n", text):
        for match in re.finditer(rf"(?<![\w/]){re.escape(test)}(?!\w|\.py)", sentence):
            yield WORD.findall(sentence[:match.start()].lower()), WORD.findall(sentence[match.end():].lower())


def asks_to_edit(before, after):
    """An edit verb within seven words before the name, with no negation or new clause between them,
    none just before the verb, and no "unchanged" just after the name. "Add" counts only as "add ... to"."""
    for index in range(len(before) - 1, max(len(before) - 8, -1), -1):
        word = before[index]
        if word in HOLDS or word in CLAUSES:
            return False
        if word in EDIT_VERBS or word in ADD_VERBS and before[-1] in ("to", "into"):
            return not HOLDS.intersection(before[max(index - 3, 0):index]) and "unchanged" not in after[:3]
    return False


def valid_forcing(check):
    try:
        return "forcing" in check and Forcing.parse(check["forcing"]) is not None
    except HarnessInputError:
        return False


def function(node_id):
    # The harness's edit guard compares a test's function source, so every case shares it.
    return node_id.split("[", 1)[0]


def edit_violations(goals, reply):
    """Regression and behavior-preserving checks run tests exactly as the base wrote them,
    so a test the change edits can only be shown by a demonstrating check (#438).
    Return substantive violations separately from uncovered edit declarations."""
    violations = []
    uncovered = []
    frozen, demonstrated = {}, set()
    for index, check in enumerate(reply["checks"]):
        for node_id in check["node_ids"]:
            if check["kind"] == "demonstrating":
                demonstrated.add(function(node_id))
            else:
                frozen.setdefault(function(node_id), (f"check-{index + 1}", check["kind"], node_id))
    subjects = [unit for check in reply['checks'] if check['kind'] == 'demonstrating'
                for unit in check.get('proof', {}).get('subjects', [])]
    edits = reply.get("edits_tests", [])
    edited = set()
    for node_id in edits:
        try:
            Check("edits", Kind.DEMONSTRATING, (node_id,))
        except ValueError:
            violations.append("edits_tests must name exact pytest node IDs.")
            continue
        edited.add(function(node_id))
        path = node_id.split("::")[0]
        if path not in reply["files"]:
            violations.append(f"A test the change edits must be in a declared file: `{path}`.")
        oversized, exists = base_definition(node_id)
        if oversized:
            violations.append(f"edits_tests names a test in a file over the {HARD_CAP_BYTES}-byte limit: {path}")
        elif not exists:
            violations.append(f"edits_tests must name a test defined at the base: `{node_id}`. "
                              "A test the change adds belongs only in a demonstrating check.")
        if function(node_id) in frozen:
            violations.append(f"`{node_id}` is in edits_tests, so a regression or behavior-preserving check "
                              "can't name it: those checks run tests exactly as the base wrote them. "
                              "Name it only in a demonstrating check.")
        if function(node_id) not in demonstrated and not any(overlaps(function(node_id), unit) for unit in subjects):
            uncovered.append(node_id)
    for goal_id, goal in enumerate(goals):
        for key, (name, kind, node_id) in frozen.items():
            if key in edited or node_id.split("::")[0] not in reply["files"]:
                continue
            test = key.rsplit("::", 1)[-1]
            found = list(mentions(goal["description"], test))
            # A heuristic: an edit verb governing the name says the goal edits the test.
            if any(asks_to_edit(before, after) for before, after in found):
                violations.append(f"Goal {goal_id} names `{test}`, and {name} freezes `{node_id}` as a {kind} "
                                  "check. If the change edits that test, list it in edits_tests and name it only "
                                  "in a demonstrating check. If it leaves the test as written there, guard that "
                                  "behavior with other existing tests.")
    return violations, uncovered


ROLE = Role(
    name="planner",
    prompt="planner.md",
    input_schema="planner-input",
    reply_schema="planner-reply",
    code_only_fields=("forecast", "execution", "violations", "decision_context", "evidence_errors", "forcing_rehearsals"),
    tools={
        "rehearse_forcing": "Rehearse exact existing base tests with a Q55 forcing through PrismaBuild before returning a forced leaf. Completed unusable candidates allow a distinct bounded proposal; identical candidates are cached. Infrastructure exits to controller recovery. Exhaustion or no usable final receipt requires cant_meet. No commands or head execution.",
        "list": "List worktree directories to locate material for the plan.",
        "read": "Read worktree text to identify files and checks for the goals.",
        "glob": "Find worktree paths using relative glob patterns for planning. The path must be a directory.",
        "grep": "Find references in worktree text to check the plan. The pattern is an RE2 regular expression, with no backreferences or lookarounds, matched per line; set literal to true to search text as written. The path may be a file or a directory.",
    },
    check_reply=check_reply,
)
