"""Compare a request's evidence with the installed tools, without substituting proof."""

import re

from tl3_harness.model import named
from tl3_harness.model import NATIVE_THREAD_ENVIRONMENT, RESERVED_ENVIRONMENT, RESERVED_PREFIXES, MAX_DELAY_SECONDS, MAX_TOTAL_DELAY_SECONDS
from tl3_workers.contracts import schema
from tl3_workers.rules import Allowed, Edit, Name, Rule, evaluate, register, render_all


EVIDENCE_FIELD = Name('field', '/evidence_contract')
EVIDENCE_ALLOWED = Allowed('type', schema('ingest-reply')['$defs']['evidence_contract']['type'])


def _missing_contract(facts):
    if ('tool_capabilities' in facts['inputs'] and isinstance(facts['reply'], dict)
            and 'evidence_contract' not in facts['reply']):
        return ({'field': EVIDENCE_FIELD},)
    return ()


CONTRACT_MISSING = register(Rule(
    'evidence.contract_missing', _missing_contract,
    lambda facts: (facts['field'],), lambda facts: EVIDENCE_ALLOWED,
    lambda facts: Edit(facts['field'], 'add', EVIDENCE_ALLOWED),
    example={'inputs': {'tool_capabilities': []}, 'reply': {}}, witness=[]), roles=('ingest', 'audit'))


# #931/Q27: this one row owns the conservative prose fallback. Full clauses
# must be known execution mechanics; an unknown or mixed clause stays mandatory.
_ROUTE_CLAUSES = (
    r"run (?:any required development checks|development tests|validation) "
    r"(?:only )?through (?:admitted prismabuild with the published entrypoints|"
    r"prismabuild at priority -10|tl3's normal admitted prismabuild `run_tests`)",
    r"read the execution policy and operating guide first",
    r"reserve aggregate demand, bound native threads, preserve assigned affinity, "
    r"and allow portable work on any eligible worker",
    r"declare aggregate cpu/memory demand and bound native threads",
    r"read each run's own verdict and log",
    r"record skips",
    r"print only allowlisted status, returncode, hashes, sizes and timestamps",
    r"read the actual terminal verdict, logs and cas receipts with allowlisted fields only",
    r"acknowledgement is not a pass",
    r"verify terminal records, logs and cas receipts",
)


def _route_only(text, entry=None, inputs=None):
    if not isinstance(text, str) or not text.strip():
        return False
    if entry is not None:
        # Methods and protected edits carry verification/scope obligations. Unknown
        # dimensions are retained even when the English looks like routing prose.
        if entry.get('methods') or entry.get('protected_files'):
            return False
        capabilities = (inputs or {}).get('tool_capabilities', [])
        executors = {'prismabuild_pytest'} | {item['executor'] for item in capabilities}
        if entry.get('tool') != 'run_tests' or entry.get('executor') not in executors:
            return False
        if any(not re.fullmatch(r'native_threads_[1-9][0-9]*', value)
               for value in entry.get('runner_features', [])):
            return False
        if set(entry.get('artifacts', [])) - {'cas_receipt', 'terminal_verdict', 'logs', 'sha'}:
            return False
    clauses = [part.strip() for part in re.split(r'[.;]\s*', text.lower()) if part.strip()]
    return bool(clauses) and all(any(re.fullmatch(pattern, clause) for pattern in _ROUTE_CLAUSES)
                                 for clause in clauses)


def _route_context(facts):
    reply, inputs = facts['reply'], facts['inputs']
    if not isinstance(reply, dict):
        return ()
    changes, context = [], []
    for field in ('evidence_contract', 'goals', 'objections'):
        entries = reply.get(field)
        if not isinstance(entries, list):
            continue
        removed, kept = [], []
        for index, entry in enumerate(entries):
            text = (entry.get('requirement') if field == 'evidence_contract' else
                    entry.get('description') if field == 'goals' else entry) if isinstance(entry, dict) else entry
            route = _route_only(text, entry if field == 'evidence_contract' and isinstance(entry, dict) else None, inputs)
            if field == 'goals' and isinstance(entry, dict) and set(entry) - {'description', 'verification'}:
                route = False
            if route:
                removed.append(Name('field', f'/{field}/{index}'))
                context.append(text)
            else:
                kept.append(entry)
        if removed:
            changes.append({'subjects': tuple(removed), 'pointer': Name('field', '/' + field), 'value': kept})
    if context and facts['role'] == 'ingest':
        if reply.get('kind') != 'brief' or not isinstance(reply.get('brief'), str):
            return ()  # An early-end reply has no context field to preserve.
        brief = reply['brief']
        for text in dict.fromkeys(context):
            if text not in brief:
                brief += '\n\n' + text
        if brief != reply['brief']:
            changes.append({'subjects': (Name('field', '/brief'),), 'pointer': Name('field', '/brief'), 'value': brief})
    if (facts['role'] == 'audit' and reply.get('verdict') == 'disagree'
            and any(item['pointer'].value == '/objections' and not item['value'] for item in changes)):
        changes.append({'subjects': (Name('field', '/verdict'),), 'pointer': Name('field', '/verdict'), 'value': 'agree'})
    return changes


_ROUTE_EXAMPLE = {'inputs': {}, 'role': 'audit', 'reply': {
    'verdict': 'disagree', 'objections': ['Read each run\'s own verdict and log.']}}
ROUTE_CONTEXT = register(Rule(
    'evidence.route_context', _route_context, lambda facts: facts['subjects'],
    lambda facts: Allowed('const', facts['value']),
    lambda facts: Edit(facts['pointer'], 'replace', Allowed('const', facts['value'])),
    example=_ROUTE_EXAMPLE, witness={'verdict': 'agree', 'objections': []}), roles=('ingest', 'audit'))


def contract_violations(inputs, reply, *, role='ingest'):
    return evaluate((CONTRACT_MISSING, ROUTE_CONTEXT),
                    {'inputs': inputs, 'reply': reply, 'role': role}, role=role)


class EvidenceContractError(ValueError):
    """A fixed, safe diagnostic for the existing reply-format repair path."""

    def __init__(self, violations=None, *, role='ingest'):
        self.rule_role = role
        self.rule_violations = (contract_violations({'tool_capabilities': []}, {})
                                if violations is None else violations)
        super().__init__(render_all(self.rule_violations, role=self.rule_role))


def check_contract(inputs, reply, *, role):
    violations = contract_violations(inputs, reply, role=role)
    if violations:
        raise EvidenceContractError(violations, role=role)


def forcing_capabilities(cpus=1):
    """Code-owned model context, independent of operator-selected executor support."""
    return {
        "delay": {"per_call_seconds": MAX_DELAY_SECONDS, "per_test_seconds": MAX_TOTAL_DELAY_SECONDS,
                  "shared_across": ["targets", "forks"], "reset": "each pytest test"},
        "reserved_environment": sorted(RESERVED_ENVIRONMENT),
        "reserved_prefixes": list(RESERVED_PREFIXES),
        "runner_environment": {name: str(cpus) for name in NATIVE_THREAD_ENVIRONMENT},
        "runner_feature": f"native_threads_{cpus}",
        "executor": "prismabuild_pytest",
        "path_depth": {"supported": False, "reason":
                       "No bounded sandbox-local temporary-root/path-depth forcing is installed. "
                       "TMPDIR/TEMP/TMP and PYTEST* remain reserved."},
    }


def forcing_guidance(cpus=1):
    contract = forcing_capabilities(cpus)
    return ("Q55 forcing environment cannot set these reserved names: "
            + ", ".join(contract["reserved_environment"])
            + ". It also cannot set any name starting with: "
            + ", ".join(contract["reserved_prefixes"])
            + ". Native-thread bounds are runner configuration: "
            + ", ".join(f"{name}={value}" for name, value in contract["runner_environment"].items())
            + ". Keep these names out of forcing.environment, including when the requested value matches. "
            f"Represent the configured bound with runner_features=[{contract['runner_feature']}] on "
            "prismabuild_pytest and retain its required run artifacts. Check tool_capabilities for support. "
            "forced_reproduction.environment describes only symptom mutations. "
            f"Delay is bounded to {contract['delay']['per_call_seconds']} seconds per call and "
            f"{contract['delay']['per_test_seconds']} seconds per pytest test, shared across targets and forks; "
            "the budget resets each pytest test. A delay patch can use an empty environment. The reviewer still judges causal relevance "
            "and the harness still requires forced base failure and head passes with and without forcing. "
            "Never claim a future run has already supplied evidence. "
            + contract["path_depth"]["reason"]
            + " If permitted environment forcings or repository patches cannot recreate the condition, "
            "report the remaining source obligation and this missing capability; do not invent a root control.")


DIMENSIONS = ('runner_features', 'artifacts', 'methods', 'protected_files')


def subject_covers(chosen, subject):
    """The selected module or definition includes the required source subject."""
    return subject == chosen or subject.startswith(chosen + '::')


def proof_applicability(goals, checks, defined=None, *, campaign=False, missing_subjects=None):
    """Check whether a method can supply its cited source goals' assertion proof."""
    required = any('proof_obligation' in goal for goal in goals)
    errors = []
    for check_index, check in enumerate(checks):
        cited = check.get('goal_ids')
        if cited is None and not required:
            continue
        if (not isinstance(cited, list) or not cited
                or any(type(index) is not int or not 0 <= index < len(goals) for index in cited)
                or len(set(cited)) != len(cited)):
            errors.append(named('proof.goal_ids', 'Proof applicability requires nonempty goal_ids naming the source goals.', dict(check=check_index, goal_ids=cited)))
            continue
        for index in cited:
            obligation = goals[index].get('proof_obligation')
            if obligation is None:
                continue
            prefix = f'Goal {index} source assertion proof ({obligation["source"]})'
            proof = check.get('proof', {})
            if (check['kind'] not in ('demonstrating', 'reproducing') or 'declared_symptom' in check
                    or campaign and obligation['observer'] == 'new'):
                errors.append(named('proof.observer_kind', prefix + ' requires an assertion observer; an old-node symptom campaign cannot discharge it.', dict(check=check_index, goal=index, description=goals[index]["description"], obligation=obligation)))
            elif campaign:
                # Rehearsal establishes old-test assertion RED. The completed plan
                # must still bind subjects and the reason before maker handoff.
                continue
            elif not proof.get('red_message', '').strip():
                errors.append(named('proof.red_missing', f'check-{check_index + 1} ' + prefix + ' requires proof.red_message binding the deterministic assertion.', dict(check=check_index, goal=index, description=goals[index]["description"], obligation=obligation, field="proof.red_message")))
            else:
                missing = {subject for subject in obligation['subjects']
                           if not any(subject_covers(chosen, subject) for chosen in proof.get('subjects', []))}
                if missing:
                    if missing_subjects is None:
                        errors.append(named('proof.subject_identify', prefix + ' must identify its subjects in proof.subjects: ' + ', '.join(obligation['subjects']), dict(check=check_index, goal=index, description=goals[index]["description"], subjects=obligation["subjects"])))
                    else:
                        missing_subjects.setdefault(check_index, set()).update(missing)
                if obligation['observer'] == 'new' and any(defined(node) for node in check['node_ids']):
                    errors.append(named('proof.new_observer', prefix + ' requires a new deterministic observer; an existing base test is insufficient.', dict(check=check_index, goal=index, description=goals[index]["description"], nodes=check["node_ids"])))
    return errors


def branch_satisfies(required, branch, context, *, prefix=False):
    if '<' not in required:
        return branch.startswith(required) if prefix else branch == required
    import re
    optional_word = required.endswith('[-<word>]')
    rest = required[:-len('[-<word>]')] if optional_word else required
    pattern = ''
    while '<' in rest:
        literal, tail = rest.split('<', 1)
        pattern += re.escape(literal)
        slot, end, rest = tail.partition('>')
        if not end:
            return False
        if slot in ('issue', 'node') and context.get(slot) is not None:
            pattern += re.escape(str(context[slot]))
        elif slot == 'word':
            pattern += '[A-Za-z0-9][A-Za-z0-9_-]*'
        else:
            return False
    pattern += re.escape(rest)
    if optional_word:
        pattern += '(?:-[A-Za-z0-9][A-Za-z0-9_-]*)?'
    return (re.match(pattern, branch) if prefix else re.fullmatch(pattern, branch)) is not None


def branch_objections(inputs, goals):
    context = inputs.get('publication_branch')
    if context is None:
        return []
    from tl3_workers.tracing import Redactor
    import json
    redactor = Redactor(None)

    def label(text):
        return json.dumps(redactor.text(text, limit=512)).replace('`', "'").replace('<', '[').replace('>', ']').replace('@', '[at]')

    errors = []
    for goal in goals:
        requirements = goal.get('publication', {})
        for field in ('branch', 'branch_prefix'):
            required = requirements.get(field)
            if required is None:
                continue
            branch = context['branch']
            satisfied = branch_satisfies(required, branch, context, prefix=field == 'branch_prefix')
            if not satisfied:
                errors.append(named('evidence.branch_conflict',
                    f'Request-scope branch conflict: {field} requires {label(required)}; '
                    f'configured template {label(context["template"])} generates {label(branch)}. '
                    'Preserve the explicit requirement and its source. Change the request scope or repository convention before planning.', dict(field=field, required=required, branch=branch)))
    return errors


def preflight(inputs, reply, *, planning=False, objections=None):
    goals = inputs.get('ingest', {}).get('goals', []) if 'verdict' in reply else reply.get('goals', [])
    branch_errors = branch_objections(inputs, goals)
    if 'verdict' in reply and branch_errors:
        reply['verdict'] = 'disagree'
        reply['objections'] = list(dict.fromkeys([*reply['objections'], *branch_errors]))
    # Old in-flight requests have no capability declaration (#372).
    if 'tool_capabilities' not in inputs:
        return
    if 'evidence_contract' not in reply:
        if objections is None:
            raise EvidenceContractError()
        objections.append(named('evidence.contract_missing',
            'Return evidence_contract with source requirements and separately marked method choices.',
            {'field': 'evidence_contract'}))
        reply['evidence_contract'] = []
    source_subjects = (reply.get('kind') == 'leaf' and any(
        goal.get('proof_obligation', {}).get('subjects') for goal in inputs.get('ingest', {}).get('goals', [])))
    if planning and (source_subjects or any(check.get('proof', {}).get('subjects') for check in reply.get('checks', []))):
        reply['evidence_contract'].append({
            'requirement': 'Run the head observer against the base subject and the head subject.',
            'provenance': {'kind': 'method'},
            'tool': 'run_tests', 'executor': 'prismabuild_pytest', 'runner_features': ['base_head'],
            'artifacts': ['cas_receipt', 'terminal_verdict'], 'methods': ['subject_observer'],
            'protected_files': []})
    if planning and any('declared_symptom' in check for check in reply.get('checks', [])):
        reply['evidence_contract'].append({
            'requirement': 'Observe the declared exception at the original node and phase in a fixed campaign.',
            'provenance': {'kind': 'method'},
            'tool': 'run_tests', 'executor': 'prismabuild_pytest',
            'runner_features': ['base_head', 'repeat', 'parallel'],
            'artifacts': ['cas_receipt', 'logs', 'sha', 'terminal_verdict'],
            'methods': ['declared_symptom'], 'protected_files': []})
    contract = []
    # Only source obligations survive stage changes. Unmarked legacy entries remain
    # mandatory because their origin cannot be recovered safely from a method name.
    for inherited, requirements in ((True, inputs.get('ingest', {}).get('evidence_contract', [])),
                                    (True, inputs.get('audit_evidence_contract', [])),
                                    (False, reply['evidence_contract'])):
        for requirement in requirements:
            if inherited and (requirement.get('provenance', {}).get('kind') == 'method'
                              or ('verdict' in reply and _route_only(requirement.get('requirement'), requirement, inputs))):
                continue
            if requirement not in contract:
                contract.append(requirement)
    errors = list(branch_errors)
    for requirement in contract:
        for method in requirement['methods']:
            if method in ('helper_target_observer', 'helper_target_observer.setup_function'):
                errors.append(named('evidence.removed_method', f'Unsupported evidence requirement: {requirement["requirement"]} '
                              f'(removed proof encoding {method}; source obligation retained).', dict(requirement=requirement, method=method)))
        candidates = []
        for capability in inputs['tool_capabilities']:
            missing = [f'{key}={requirement[key]}' for key in ('tool', 'executor')
                       if requirement[key] != capability[key]]
            for key in DIMENSIONS:
                missing.extend(f'{key}={value}' for value in requirement[key]
                               if value not in capability[key])
            candidates.append(missing)
        if not candidates or all(candidates):
            missing = min(candidates, key=len) if candidates else [
                f'tool={requirement["tool"]}', f'executor={requirement["executor"]}']
            errors.append(named('evidence.unsupported', f'Unsupported evidence requirement: {requirement["requirement"]} '
                          f'({", ".join(missing)}).', dict(requirement=requirement, missing=missing)))
    reply['evidence_contract'] = contract
    reply['evidence_errors'] = list(dict.fromkeys(errors))
    if planning and errors and objections is not None:
        objections.extend(errors)
    if planning and errors and objections is None:
        planning_refusal(reply, errors)


def planning_refusal(reply, errors):
    """Project a completed refusal only after the planner collected all objections."""
    if reply.get('kind') == 'split':
        reply['files'] = list(dict.fromkeys(path for piece in reply['pieces'] for path in piece['files']))
    kept = {key: value for key, value in reply.items()
            if key in {'files', 'forecast', 'violations', 'evidence_contract',
                       'evidence_errors', 'protected_paths', 'memory_ids', 'memory_flags',
                       'execution', 'forcing_rehearsals', 'decision_context'}}
    reply.clear()
    reply.update(kept, kind='cant_meet', files=kept.get('files', []),
                 report='\n'.join(dict.fromkeys(errors)))
