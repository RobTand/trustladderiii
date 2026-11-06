"""Report plan measurements and rule violations to the controller."""

from copy import deepcopy
import json
from pathlib import Path, PurePosixPath
import re
from subprocess import CalledProcessError

from jsonschema import Draft202012Validator

from tl3_workers.roles.base import Role
from tl3_workers.evidence import preflight, proof_applicability, subject_covers, planning_refusal
from tl3_workers.forecast import Forecaster, HARD_CAP_BYTES, MAX_EXECUTION_PIECES, execution_forecast
from tl3_harness.model import Diagnostic, named, contextual
from tl3_harness import Check, Forcing, HarnessInputError, Kind, Proof
from tl3_harness.proof import overlaps
from tl3_harness.limits import validate_check_plan
from tl3_harness.trees import git, test_function_source
from tl3_workers.tools import Worktree
from tl3_workers.generated_scope import configured_outputs
from tl3_workers.recall import check_citations
from tl3_workers.turns import InvalidReply, Unrepairable, repaired


from tl3_workers.roles.planner_rules import inventory as rules


def same(left, right):
    # Python equality would allow True and 1, or 1.0 and 1, to exchange identities.
    return type(left) is type(right) and (
        left.keys() == right.keys() and all(same(left[k], right[k]) for k in left)
        if isinstance(left, dict) else
        len(left) == len(right) and all(same(a, b) for a, b in zip(left, right))
        if isinstance(left, (list, tuple)) else left == right)


def wire(diagnostic):
    if not isinstance(diagnostic, Diagnostic):
        raise TypeError('Unnamed plan-check diagnostic')
    return {'class': rules()[diagnostic.rule]['class'], 'rule': diagnostic.rule,
            'items': identities(diagnostic.items), 'diagnostic': str(diagnostic)}


def identities(items):
    result = []
    for key, value in sorted(items.items()):
        if key in {'description', 'descriptions', 'source_goals'}:
            if key == 'source_goals':
                result.extend(f'goal {i}' for i in range(len(value)))
            continue
        if key == 'check':
            result.append(f'check-{value + 1}' if type(value) is int else str(value))
        elif key == 'obligation':
            result.extend(identities({k: v for k, v in value.items() if k != 'quote'}))
        elif key == 'goal':
            result.append(f'goal {value}:')
        elif key == 'goals':
            result.extend(f'goal {goal}:' for goal in value)
        else:
            result.append(key + '=' + json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(',', ':')))
    return result or ['plan']


def terminal(origin, latest, completed, candidate, *, attempted, unavailable, malformed=False):
    """Preserve the rejection when correction ends, including malformed corrections."""
    reply = deepcopy(completed if completed is not None else origin.completed_reply)
    remaining = list(latest.objections) if latest is not None else []
    if not remaining and reply['kind'] == 'split':
        remaining = list(origin.objections)
    guards = []
    if attempted:
        if malformed:
            guards = [named('guard.variant', 'Planner correction failed reply validation.', {'field': 'reply'})]
        else:
            guards = origin.contract.check(candidate)
        if reply['kind'] == 'cant_meet':
            remaining = [*origin.objections, *remaining]
        remaining.extend(guards)
    if not remaining:
        return reply, None
    # Original and final diagnostics remain complete even when the prompt clips
    # optional descriptions. The wire's objections are the still-standing set.
    reply['violations'] = list(dict.fromkeys([
        *(str(item) for item in origin.objections), *reply.get('violations', []),
        *(str(item) for item in guards)]))
    if origin.missing_goals or latest is not None and latest.missing_goals:
        reply['violations'] = [item for item in reply['violations']
                               if not item.startswith('Planner coverage correction attempts:')]
        reply['violations'].append(f'Planner coverage correction attempts: {int(attempted)}.')
    if attempted and hasattr(origin.contract, 'manifest'):
        remaining = [named(item.rule, str(item), {**item.items, 'obligation_manifest': origin.contract.manifest}) for item in remaining]
    if not attempted and unavailable == 'message_limit':
        reply['violations'].append('Planner declarations exceed the existing correction message limit.')
    if not attempted and unavailable == 'input_allowance':
        reply['violations'].append('Planner correction unavailable: the existing input allowance has no room for a correction turn.')
    evidence = {'first_objections': [wire(item) for item in origin.objections],
                'final_objections': [wire(item) for item in latest.objections] if latest is not None and not malformed else [],
                'constraint_failures': [wire(item) for item in guards],
                'attempts': int(attempted), 'unavailable_reason': None if attempted else unavailable,
                'binding': {'base_sha': origin.contract.inputs.get('base_sha')}}
    reply['violations'].append('Plan-check evidence: ' + json.dumps(evidence, ensure_ascii=False, sort_keys=True))
    repair = {'status': 'attempted'} if attempted else {'status': 'unavailable', 'reason': unavailable}
    provenance = {'objections': list({json.dumps(wire(item), sort_keys=True): wire(item)
                                     for item in remaining}.values()), 'repair': repair}
    return reply, provenance


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
            f"backstop {execution['allowed_tool_turns']}. "
            f"Components: {components}.")


class PlanConsistencyError(ValueError):
    """A complete planner refusal, rendered and guarded by registered rows."""

    rule_role = 'planner'

    def __init__(self, raw, completed, diagnostics, inputs, *, base_nodes=(), base_sources=None):
        from tl3_workers.roles.planner_rules import violations
        from tl3_workers.rules import render_all

        self.raw_draft = deepcopy(raw)
        self.completed_reply = deepcopy(completed)
        self.objections = tuple(diagnostics)
        self.inputs = deepcopy(inputs)
        self.missing_goals = {g: inputs['ingest']['goals'][g]['description']
                              for item in diagnostics if item.rule == 'goal.coverage' for g in item.items['goals']}
        self.node_ids = tuple(identity for item in diagnostics for identity in identities(item.items))
        self.rule_violations = violations({
            'reply': raw, 'draft': raw, 'completed': completed, 'diagnostics': diagnostics,
            'inputs': inputs, 'base_nodes': base_nodes, 'base_sources': base_sources or {}})
        self.repair_text = render_all(self.rule_violations, role='planner')
        self.unavailable_reason = None if inputs.get('repair_plan_consistency', False) else 'disabled'
        self.contract = self
        super().__init__(self.repair_text)

    def check(self, candidate):
        from tl3_workers.rules import PLANNER, permits

        # An honest terminal reply is available after any bounded edit. CantMeet
        # rows select that same registered terminal predicate in permits.
        if PLANNER.predicate(candidate) or permits(self.rule_violations, self.raw_draft, candidate, role='planner'):
            if candidate.get('kind') == 'split' and self.raw_draft.get('kind') == 'leaf':
                original = self.raw_draft
                self.manifest = {
                    'base': self.inputs.get('base_sha'), 'goals': deepcopy(self.inputs['ingest']['goals']),
                    'checks': deepcopy(original.get('checks', [])), 'original_pieces': [],
                    'edits_tests': deepcopy(original.get('edits_tests', [])), 'files': original['files'],
                    'evidence_contract': deepcopy(self.completed_reply.get('evidence_contract', [])),
                    'forecast': deepcopy(self.completed_reply['forecast']), 'execution': deepcopy(self.completed_reply['execution']),
                    'parent_estimates': {k: original[k] for k in ('estimated_tool_turns', 'estimated_exploration_turns') if k in original},
                    'mapping': [{'check': index, 'pieces': [i for i, piece in enumerate(candidate['pieces'])
                                 if set(check['goal_ids']) <= set(piece['goal_ids']) and
                                 {node.split('::')[0] for node in check['node_ids']} <= set(piece['files'])]}
                                for index, check in enumerate(original.get('checks', []))]}
            return []
        return [named('guard.fields_changed', 'Correction changes declarations outside the named objections.', {'paths': ['reply']})]

    def constrain(self, reply, raw=None):
        candidate = raw if raw is not None else {key: value for key, value in reply.items()
            if key not in ROLE.code_only_fields}
        errors = self.check(candidate)
        reply['violations'].extend(str(item) for item in errors)
        return errors


def proof_omissions(reply):
    """The proof schema's required keys are the only declaration omissions admitted here."""
    if not isinstance(reply, dict) or reply.get('kind') != 'leaf' or not isinstance(reply.get('checks'), list):
        return {}
    required = set(PROOF_DECLARATION)
    missing = {}
    for index, check in enumerate(reply['checks']):
        if (not isinstance(check, dict) or check.get('kind') not in {'demonstrating', 'reproducing'}
                or 'declared_symptom' in check or not isinstance(check.get('proof', {}), dict)):
            continue
        absent = required - check.get('proof', {}).keys()
        if absent:
            missing[index] = absent
    return missing


# A validation-only placeholder never enters an accepted plan or a proof run.
PROOF_DECLARATION = Proof((), (), 'undeclared assertion message').as_json()


def proof_for_validation(check):
    proof = check.get('proof', {})
    red = proof.get('red_message', '')
    return {**PROOF_DECLARATION, **proof, **({'red_message': PROOF_DECLARATION['red_message']}
        if isinstance(red, str) and not red.strip() else {})}


def planner_schema_reply(reply):
    """Keep proof omissions and blank messages inside the semantic repair budget."""
    missing = proof_omissions(reply)
    if not isinstance(reply, dict) or reply.get('kind') != 'leaf':
        return reply
    result = deepcopy(reply)
    for index, check in enumerate(result.get('checks', [])):
        if index in missing:
            check['proof'] = proof_for_validation(check)
        if isinstance(check.get('proof'), dict) and isinstance(check['proof'].get('red_message'), str) and not check['proof']['red_message'].strip():
            check['proof']['red_message'] = PROOF_DECLARATION['red_message']
    return result


def check_reply(inputs, reply, *, rehearsals=None, plan_errors=None):
    if rehearsals is not None:
        rehearsals.raise_infrastructure()
    Draft202012Validator(EXPLORATION_DECLARATION).validate(reply)
    raw_draft = deepcopy(reply)
    early = []
    supplied = [key for key in ('forecast', 'violations', 'decision_context', 'forcing_rehearsals', 'evidence_errors') if key in reply]
    if supplied:
        early.append(named('plan.code_evidence', 'only code may supply planning evidence', {'keys': supplied}))
        for key in supplied:
            reply.pop(key)
    preflight(inputs, reply, planning=True, objections=early)
    check_citations(inputs, reply, objections=early)
    goals = set(range(len(inputs["ingest"]["goals"])))
    violations = ([named('plan.execution_owned', "Execution forecasts are supplied by worker code; omit execution from the model reply.", dict(field="execution"))]
                  if reply.pop("execution", None) is not None else [])
    violations[:0] = early
    uncovered = []
    missing_forced = []
    missing_forced_checks = {}
    selectors = {}
    selector_errors = set()
    missing_proof = proof_omissions(reply)
    missing_subjects = {}
    missing_goals = {}
    paths = set()
    base_nodes = set()
    base_sources = {}
    def observed_base(node):
        definition = base_definition(node, inputs.get('base_sha'))
        if getattr(definition, 'source', None) is not None:
            base_sources[node.split('::')[0]] = definition.source
        if definition[1]:
            base_nodes.add(node)
        return definition
    if reply["kind"] == "leaf":
        if inputs["must_split"]:
            violations.append(named('leaf.must_split', "Must split after running out of room.", dict(must_split=inputs["must_split"])))
        if not reply["files"]:
            violations.append(named('leaf.files_empty', "A leaf must declare its files.", dict(files=reply["files"], source_goals=inputs["ingest"]["goals"])))
        if not reply["no_design_questions"]:
            violations.append(named('leaf.design_open', "A leaf must resolve every design question.", dict(no_design_questions=reply["no_design_questions"], source_goals=inputs["ingest"]["goals"])))
        if not reply["lands_on_its_own"]:
            violations.append(named('leaf.not_independent', "A leaf must land on its own.", dict(lands_on_its_own=reply["lands_on_its_own"], source_goals=inputs["ingest"]["goals"])))
        for index, check in enumerate(reply["checks"]):
            if 'proof' in check or index in missing_proof:
                if isinstance(check.get('proof'), dict) and 'red_message' in check['proof'] and not check['proof']['red_message'].strip():
                    obligation = next((inputs['ingest']['goals'][g]['proof_obligation']
                        for g in check['goal_ids'] if type(g) is int and 0 <= g < len(inputs['ingest']['goals'])
                        and 'proof_obligation' in inputs['ingest']['goals'][g]), None)
                    if obligation is None:
                        violations.append(named('proof.red_missing', f'check-{index + 1} requires proof.red_message binding the deterministic assertion.', dict(check=index, field='proof.red_message', obligation={})))
                try:
                    proof = Proof.parse(proof_for_validation(check) if index in missing_proof or not check.get('proof', {}).get('red_message', '').strip() else check['proof'])
                    for unit in (*proof.subjects, *proof.observer_support):
                        if unit.split('::')[0] not in reply['files']:
                            violations.append(named('proof.file_missing', f'Proof source must be in declared files: {unit}', dict(check=index, unit=unit, path=unit.split('::')[0])))
                except HarnessInputError as error:
                    violations.append(contextual(error, check=index, declaration=check))
        covered = set()
        forcings = {}
        exact_nodes = set()
        for index, check in enumerate(reply["checks"]):
            if "forcing" in check:
                # #393: a forced reproduction is the red for a deflake.
                if check["kind"] != "demonstrating":
                    violations.append(named('check.forcing_kind', "Only a demonstrating check can carry a forcing.", dict(check=index, kind=check["kind"], forcing=check["forcing"])))
                try:
                    forcings[index] = Forcing.parse(check["forcing"])
                except HarnessInputError as error:
                    cause = contextual(error, check=index, declaration=check["forcing"])
                    violations.append(named(cause.rule, f"Invalid forcing: {error}", cause.items))
            if not check["node_ids"]:
                violations.append(named('check.tests_empty', "Every check must name at least one test.", dict(check=index, goal_ids=check["goal_ids"])))
            if not check["goal_ids"]:
                violations.append(named('check.goals_empty', "Every check must cite at least one goal.", dict(check=index, nodes=check["node_ids"])))
            for node_id in check["node_ids"]:
                try:
                    Check("planned", Kind(check["kind"]), (node_id,))
                except HarnessInputError as error:
                    selector_errors.add(str(error))
                    selectors.setdefault(index, []).append(node_id)
                    continue
                exact_nodes.add(node_id)
                # A forced check runs the old test on the base, so it must exist there too.
                if check["kind"] in {"regression", "behavior-preserving"} or ("forcing" in check and not check.get("proof", {}).get("subjects")) or "declared_symptom" in check:
                    path = node_id.split("::")[0]
                    definition = observed_base(node_id)
                    oversized, exists = definition
                    if oversized:
                        violations.append(named('check.base_file_large', f"Existing check's test file is over the {HARD_CAP_BYTES}-byte limit: {path}", dict(check=index, node=node_id, path=path, bytes=getattr(definition, "size", None), limit=HARD_CAP_BYTES)))
                    elif not exists:
                        if check["kind"] == "demonstrating" and "forcing" in check:
                            missing_forced.append(node_id)
                            missing_forced_checks.setdefault(index, []).append(node_id)
                        else:
                            guidance = (' A new assertion observer needs a demonstrating check; adding it only '
                                        'at head cannot repair an original-base regression.'
                                        if check['kind'] in {'regression', 'behavior-preserving'} else '')
                            violations.append(named('check.base_missing', f"Existing check must name a test defined at the base: {node_id}.{guidance}", dict(check=index, kind=check["kind"], node=node_id, base=inputs.get("base_sha"))))
            cited = set(check["goal_ids"])
            if not cited <= goals:
                violations.append(named('check.goal_unknown', "A check cites an unknown goal.", dict(check=index, unknown=sorted(cited-goals))))
            covered.update(cited)
        applicable = [(index, check) for index, check in enumerate(reply['checks'])
                      if set(check['node_ids']) <= exact_nodes]
        omissions = {}
        applicability_errors = proof_applicability(inputs['ingest']['goals'], [check for _, check in applicable],
            lambda node: observed_base(node)[1], missing_subjects=omissions)
        for error in applicability_errors:
            items = {**error.items, 'check': applicable[error.items['check']][0]}
            text = str(error).replace(f'check-{error.items["check"] + 1} ', f'check-{items["check"] + 1} ', 1) if error.rule == 'proof.red_missing' else str(error)
            violations.append(named(error.rule, text, items))
        missing_subjects = {applicable[index][0]: subjects for index, subjects in omissions.items()}
        edit_errors, uncovered = edit_violations(inputs["ingest"]["goals"], reply, inputs.get('base_sha'))
        violations.extend(edit_errors)
        planned_checks = []
        for i, check in enumerate(reply['checks']):
            if not check['node_ids']:
                continue
            try:
                planned_checks.append(Check(f'check-{i + 1}', Kind(check['kind']), tuple(check['node_ids']),
                                            forcings.get(i) if check['kind'] == 'demonstrating' else None,
                                            Proof.parse(proof_for_validation(check))
                                            if 'proof' in check or i in missing_proof else None,
                                            check.get('declared_symptom')))
            except ValueError as error:
                if str(error) not in selector_errors:
                    violations.append(contextual(error, check=i, declaration=check))
        try:
            validate_check_plan(tuple(planned_checks))
        except ValueError as error:
            violations.append(contextual(error, checks=reply['checks']))
        required = {i for i, goal in enumerate(inputs["ingest"]["goals"])
                    if goal.get("stage", "maker") != "publishing"}
        missing_goals = {index: inputs['ingest']['goals'][index]['description']
                         for index in sorted(required - covered)}
        paths.update(reply["files"])
    elif reply["kind"] == "split":
        covered = set()
        for piece_index, piece in enumerate(reply["pieces"]):
            if not piece["goal_ids"]:
                violations.append(named('split.goal_empty', "Every piece must cite at least one goal.", dict(piece=piece_index, source_goals=inputs["ingest"]["goals"])))
            cited = set(piece["goal_ids"])
            if not cited <= goals:
                violations.append(named('split.goal_unknown', "A piece cites an unknown goal.", dict(piece=piece_index, unknown=sorted(cited-goals))))
            covered.update(cited)
            paths.update(piece["files"])
        if len(reply["pieces"]) < 2:
            violations.append(named('split.too_few', "A split must propose at least two pieces.", dict(count=len(reply["pieces"]), minimum=2)))
        if covered != goals:
            violations.append(named('split.goal_missing', "A split must preserve every goal.", dict(missing=sorted(goals-covered), source_goals=inputs["ingest"]["goals"])))
    else:
        paths.update(reply["files"])

    if reply["kind"] in {"leaf", "split"}:
        for path in sorted(paths):
            if any(PurePosixPath(path).full_match(pattern) for pattern in inputs["protected_paths"]):
                violations.append(named('path.protected', f"Declared file matches a protected path: {path}", dict(path=path, patterns=inputs["protected_paths"])))

    if reply["kind"] in {"leaf", "split"} and inputs["forecast"].get("generator"):
        with Worktree(".") as tree:
            outputs = configured_outputs(tree, inputs["forecast"], inputs["protected_paths"])
        paths.update(outputs)
        if reply["kind"] == "leaf":
            reply["files"] = sorted(paths)
        else:
            for piece_index, piece in enumerate(reply["pieces"]):
                piece["files"] = sorted(set(piece["files"]) | set(outputs))

    forecaster = Forecaster(inputs["forecast"])
    forecast = forecaster.measure(inputs["ingest"]["brief"], paths)
    check_count = (len(reply["checks"]) if reply["kind"] == "leaf" else len(goals))
    execution = execution_forecast(paths, check_count, declared=reply.get("estimated_tool_turns", 0),
                                   exploration=reply.get("estimated_exploration_turns"),
                                   config=inputs["forecast"])
    if reply["kind"] == "leaf" and execution["exploration_declared"] and not execution["fits"]:
        violations.append(named('leaf.execution_fit', f"The leaf exceeds the execution budget: {execution_budget_details(execution)} "
                          "Preserve every obligation in bounded pieces with reason execution_budget before maker work.", dict(execution=execution)))
    # A file the tools can't use is a plan problem, not a crash (#321).
    for error in forecaster.problems:
        violations.append(named(error.rule, f"Declared file can't be used: {error}. Leave it out.", dict(path=error.path, reason=str(error.reason), bytes=error.size)))
    if reply["kind"] == "leaf" and not forecast["fits"]:
        violations.append(named('leaf.token_fit', "The leaf forecast does not fit. Propose smaller pieces.", dict(forecast=forecast)))
    if reply["kind"] == "split":
        reason = reply["reason"]
        if reason == "multiple_repos":
            violations.append(named('split.multiple_repos', "This request is in one repository.", dict(reason=reason)))
        if reason == "out_of_room" and not inputs["must_split"]:
            violations.append(named('split.no_out_of_room', "No maker has run out of room.", dict(reason=reason, must_split=inputs["must_split"])))
        if reason == "does_not_fit" and forecast["fits"]:
            violations.append(named('split.forecast_fits', "The forecast fits. Splitting needs a permitted reason.", dict(reason=reason, forecast=forecast)))
        if reason == "execution_budget":
            if execution["fits"]:
                violations.append(named('split.execution_fits', "The execution estimate fits; execution_budget is not a permitted reason for this plan.", dict(reason=reason, execution=execution)))
            if len(reply["pieces"]) > MAX_EXECUTION_PIECES:
                violations.append(named('split.too_many', f"Execution-budget decomposition permits at most {MAX_EXECUTION_PIECES} pieces.", dict(count=len(reply["pieces"]), maximum=MAX_EXECUTION_PIECES)))
        pieces = []
        for piece_index, piece in enumerate(reply["pieces"]):
            child = forecaster.measure(piece["brief"], piece["files"])
            estimate = execution_forecast(piece["files"], len(piece["goal_ids"]),
                                          declared=piece.get("estimated_tool_turns", 0),
                                          exploration=piece.get("estimated_exploration_turns"),
                                          config=inputs["forecast"])
            pieces.append(estimate)
            for error in forecaster.problems:
                violations.append(named(error.rule, f"Declared file can't be used: {error}. Leave it out.", dict(path=error.path, reason=str(error.reason), bytes=error.size)))
            if reason == "execution_budget":
                if not estimate["fits"] or estimate["estimated_tool_turns"] >= execution["estimated_tool_turns"]:
                    violations.append(named('split.execution_piece', "Every execution-budget piece must fit the execution backstop and reduce demand. "
                                      + execution_budget_details(estimate), dict(piece=piece_index, estimate=estimate, parent=execution)))
                if not child["fits"]:
                    violations.append(named('split.token_piece', "Every execution-budget piece must also fit the token forecast.", dict(piece=piece_index, forecast=child)))
            elif child["tokens"] >= forecast["tokens"]:
                violations.append(named('split.not_smaller', "Every proposed piece must have a smaller forecast than its parent.", dict(piece=piece_index, child=child, parent=forecast)))
        execution["pieces"] = pieces
        execution["aggregate_tool_turns"] = sum(piece["estimated_tool_turns"] for piece in pieces)
        execution["aggregate_allowed_tool_turns"] = sum(piece["allowed_tool_turns"] for piece in pieces)
    failed_work = False
    if rehearsals is not None and rehearsals.failure:
        failed_work = True
        violations.append(named(rehearsals.failure.get('rule', 'rehearsal.exhausted'), rehearsals.failure['message'], {'receipt': rehearsals.failure}))
    elif rehearsals is not None and reply['kind'] == 'cant_meet':
        rejected = next((item for item in reversed(rehearsals.candidates) if item['status'] == 'refused'), None)
        if rejected is not None:
            failed_work = True
            violations.append(named(rejected.get('rule', 'rehearsal.missing_receipt'), rejected['message'], {'receipt': rejected}))
    consistency_facts = (
        [('edit.uncovered', {'nodes': [node]}) for node in uncovered]
        + [('forced.base_missing', {'check': i, 'nodes': nodes}) for i, nodes in missing_forced_checks.items()]
        + [('check.selector_inexact', {'check': i, 'nodes': nodes}) for i, nodes in selectors.items()]
        + [('proof.omitted', {'check': i, 'fields': sorted(fields)}) for i, fields in missing_proof.items()]
        + [('proof.subject_missing', {'check': i, 'subjects': sorted(subjects)}) for i, subjects in missing_subjects.items()]
        + ([('goal.coverage', {'goals': sorted(missing_goals), 'descriptions': missing_goals})] if missing_goals else []))
    violations.extend(named(code, code + ': ' + '; '.join(identities(items)), items) for code, items in consistency_facts)
    if reply['kind'] == 'leaf':
        valid = [check for check in reply['checks'] if valid_forcing(check)]
        if rehearsals is not None:
            violations.extend(contextual(text, checks=[i for i, check in enumerate(reply['checks']) if valid_forcing(check)])
                              for text in rehearsals.violations(inputs.get('base_sha'), valid))
        elif valid:
            violations.append(named('rehearsal.no_ledger_receipt', 'Q55 check lacks a matching forcing rehearsal receipt for the exact base, selectors and forcing.', dict(checks=[i for i, check in enumerate(reply["checks"]) if valid_forcing(check)], base=inputs.get("base_sha"))))
    if rehearsals is not None and (rehearsals.candidates or rehearsals.failure):
        reply['forcing_rehearsals'] = deepcopy(rehearsals.candidates + ([rehearsals.failure] if rehearsals.failure else []))
    if "decision_context" in inputs:
        reply["decision_context"] = inputs["decision_context"]
    reply["forecast"] = forecast
    reply["execution"] = execution
    reply["violations"] = list(dict.fromkeys(violations))
    if reply.get('evidence_errors'):
        planning_refusal(reply, reply['evidence_errors'])
    if violations:
        # Record before deduplicating human text; identical messages can name
        # different checks, pieces or fields.
        repair_error = PlanConsistencyError(raw_draft, reply, violations, inputs,
                                            base_nodes=base_nodes, base_sources=base_sources)
        if failed_work:
            repair_error.unavailable_reason = 'failed_work'
        if missing_goals:
            reply['violations'].append('Planner coverage correction attempts: 0.')
            repair_error.completed_reply = deepcopy(reply)
        if repair_error.unavailable_reason is None:
            raise repair_error
        if plan_errors is not None:
            plan_errors.append(repair_error)
    return None


class BaseDefinition(tuple):
    def __new__(cls, oversized, exists, size=None, source=None):
        result = super().__new__(cls, (oversized, exists))
        result.size = size
        result.source = source
        return result

    def __reduce__(self):
        return BaseDefinition, (*self, self.size, self.source)


def base_definition(node_id, base_sha=None):
    """Whether the base defines the test, after whether its file is over the hard cap."""
    # The whole module is parsed to find the test, so the
    # declared-file hard cap bounds it, not a page size.
    size = None
    try:
        if base_sha is not None:
            identity = base_sha + ':' + node_id.split('::')[0]
            size = int(git(Path('.'), 'cat-file', '-s', identity))
            if size > HARD_CAP_BYTES:
                return BaseDefinition(True, False, size)
            source = git(Path('.'), 'cat-file', 'blob', identity).decode('utf-8')
        else:
            with Worktree(".") as tree, tree._file(node_id.split("::")[0]) as stream:
                source = stream.read(HARD_CAP_BYTES + 1)
    except (OSError, ValueError, UnicodeError, CalledProcessError):
        return BaseDefinition(False, False, size)
    size = len(source.encode("utf-8"))
    if size > HARD_CAP_BYTES:
        return BaseDefinition(True, False, size)
    return BaseDefinition(False, test_function_source(source, node_id) is not None, size, source)


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


def edit_violations(goals, reply, base_sha=None):
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
            violations.append(named('edit.selector', "edits_tests must name exact pytest node IDs.", dict(node=node_id)))
            continue
        edited.add(function(node_id))
        path = node_id.split("::")[0]
        if path not in reply["files"]:
            violations.append(named('edit.file_missing', f"A test the change edits must be in a declared file: `{path}`.", dict(node=node_id, path=path)))
        definition = base_definition(node_id, base_sha)
        oversized, exists = definition
        if oversized:
            violations.append(named('edit.file_large', f"edits_tests names a test in a file over the {HARD_CAP_BYTES}-byte limit: {path}", dict(node=node_id, path=path, bytes=getattr(definition, "size", None), limit=HARD_CAP_BYTES)))
        elif not exists:
            violations.append(named('edit.base_missing', f"edits_tests must name a test defined at the base: `{node_id}`. "
                              "A test the change adds belongs only in a demonstrating check.", dict(node=node_id)))
        if function(node_id) in frozen:
            violations.append(named('edit.frozen', f"`{node_id}` is in edits_tests, so a regression or behavior-preserving check "
                              "can't name it: those checks run tests exactly as the base wrote them. "
                              "Name it only in a demonstrating check.", dict(node=node_id, frozen=frozen[function(node_id)])))
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
                violations.append(named('edit.goal_frozen', f"Goal {goal_id} names `{test}`, and {name} freezes `{node_id}` as a {kind} "
                                  "check. If the change edits that test, list it in edits_tests and name it only "
                                  "in a demonstrating check. If it leaves the test as written there, guard that "
                                  "behavior with other existing tests.", dict(goal=goal_id, description=goal["description"], check=name, node=node_id)))
    return violations, uncovered


class PlannerContract:
    """The planner's one model-loop contract hook; owns #832 provenance."""

    def __init__(self, inputs, worktree=None):
        self.inputs, self.worktree = inputs, worktree
        self.provenance = None
        self.origin = self.latest = self.completed = self.candidate = None
        self.calls = 0
        self.origin_call = None

    def check(self, reply, *, plan_errors=None):
        from tl3_workers.contracts import validate
        validate('planner-reply', planner_schema_reply(reply))
        check_reply(self.inputs, reply, rehearsals=getattr(self.worktree, 'forcing_rehearsals', None),
                    plan_errors=plan_errors)
        return reply

    def run(self, control, history, ask):
        from jsonschema import ValidationError
        from tl3_workers.contracts import read_reply

        ledger = getattr(self.worktree, 'forcing_rehearsals', None)

        def checked(text):
            self.calls += 1
            if ledger is not None:
                ledger.raise_infrastructure()
                if ledger.failure:
                    return refused(ledger.failure)
            captured = []
            try:
                self.candidate = read_reply(text)
                result = self.check(deepcopy(self.candidate), plan_errors=captured)
            except Unrepairable:
                raise
            except (ValueError, ValidationError) as error:
                if ledger is not None:
                    rejected = next((item for item in reversed(ledger.candidates) if item['status'] == 'refused'), None)
                    if rejected is not None:
                        return refused(rejected)
                if isinstance(error, PlanConsistencyError):
                    capture(error)
                    self.completed = error.completed_reply
                raise
            if captured:
                capture(captured[-1])
            else:
                self.latest = None
            self.completed = result
            return result

        def capture(error):
            self.latest = error
            if self.origin is None:
                self.origin, self.origin_call = error, self.calls

        def refused(failure):
            self.candidate = {'kind': 'cant_meet', 'files': [], 'evidence_contract': [], 'report': failure['message']}
            captured = []
            self.completed = self.check(deepcopy(self.candidate), plan_errors=captured)
            capture(captured[-1])
            return self.completed

        failed = None
        try:
            reply = repaired(control, history, ask, checked, guard_after_check=True)
        except (PlanConsistencyError, InvalidReply) as error:
            if self.origin is None:
                raise
            failed, reply = error, self.completed
        if self.origin is not None:
            attempted = self.origin_call == 1 and control.repair_attempts == 1
            malformed = failed is not None and isinstance(failed, InvalidReply) and not isinstance(failed.errors[-1], PlanConsistencyError)
            guards = self.origin.check(self.candidate) if attempted and not malformed else []
            if self.latest is not None or guards or malformed or reply['kind'] in {'cant_meet', 'split'}:
                unavailable = self.origin.unavailable_reason or control.repair_unavailable or 'format_allowance_consumed'
                reply, self.provenance = terminal(self.origin, self.latest, reply, self.candidate,
                    attempted=attempted, unavailable=unavailable, malformed=malformed)
        return reply


ROLE = Role(
    reply_contract=PlannerContract,
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
