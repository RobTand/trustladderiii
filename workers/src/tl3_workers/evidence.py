"""Compare a request's evidence with the installed tools, without substituting proof."""

from tl3_harness.model import NATIVE_THREAD_ENVIRONMENT, RESERVED_ENVIRONMENT, RESERVED_PREFIXES


def forcing_capabilities(cpus=1):
    """Code-owned model context, independent of operator-selected executor support."""
    return {
        "reserved_environment": sorted(RESERVED_ENVIRONMENT),
        "reserved_prefixes": list(RESERVED_PREFIXES),
        "runner_environment": {name: str(cpus) for name in NATIVE_THREAD_ENVIRONMENT},
        "runner_feature": f"native_threads_{cpus}",
        "executor": "prismabuild_pytest",
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
            "A delay patch can use an empty environment. The reviewer still judges causal relevance "
            "and the harness still requires forced base failure and head passes with and without forcing. "
            "Never claim a future run has already supplied evidence.")


DIMENSIONS = ('runner_features', 'artifacts', 'methods', 'protected_files')


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
            satisfied = branch == required if field == 'branch' else branch.startswith(required)
            if not satisfied:
                errors.append(
                    f'Request-scope branch conflict: {field} requires {label(required)}; '
                    f'configured template {label(context["template"])} generates {label(branch)}. '
                    'Preserve the explicit requirement and its source. Change the request scope or repository convention before planning.')
    return errors


def preflight(inputs, reply, *, planning=False):
    goals = inputs.get('ingest', {}).get('goals', []) if 'verdict' in reply else reply.get('goals', [])
    branch_errors = branch_objections(inputs, goals)
    if 'verdict' in reply and branch_errors:
        reply['verdict'] = 'disagree'
        reply['objections'] = list(dict.fromkeys([*reply['objections'], *branch_errors]))
    # Old in-flight requests have no capability declaration (#372).
    if 'tool_capabilities' not in inputs:
        return
    if 'evidence_contract' not in reply:
        raise ValueError('derive evidence_contract from every mandatory execution requirement')
    helper_checks = [check for check in reply.get('checks', []) if 'helper_evidence' in check] if planning else []
    if helper_checks:
        reply['evidence_contract'].append({
            'requirement': 'Observe the real old and new test helper with identical new regression assertions.',
            'tool': 'run_tests', 'executor': 'prismabuild_pytest', 'runner_features': ['base_head'],
            'artifacts': ['cas_receipt', 'terminal_verdict'], 'methods': ['helper_target_observer'],
            'protected_files': []})
        if any('$setup' in value for check in helper_checks
               for case in check['helper_evidence'].get('failure_cases', [])
               for value in [*case['args'], *case['kwargs'].values()] if isinstance(value, dict)):
            reply['evidence_contract'].append({
                'requirement': 'Construct helper arguments with unchanged existing repository setup functions.',
                'tool': 'run_tests', 'executor': 'prismabuild_pytest', 'runner_features': ['base_head'],
                'artifacts': ['cas_receipt', 'terminal_verdict'],
                'methods': ['helper_target_observer.setup_function'], 'protected_files': []})
    contract = []
    for requirements in (inputs.get('ingest', {}).get('evidence_contract', []),
                         inputs.get('audit_evidence_contract', []), reply['evidence_contract']):
        for requirement in requirements:
            if requirement not in contract:
                contract.append(requirement)
    errors = list(branch_errors)
    for requirement in contract:
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
            errors.append(f'Unsupported evidence requirement: {requirement["requirement"]} '
                          f'({", ".join(missing)}).')
    reply['evidence_contract'] = contract
    reply['evidence_errors'] = list(dict.fromkeys(errors))
    if planning and errors:
        if reply.get('kind') == 'split':
            reply['files'] = list(dict.fromkeys(path for piece in reply['pieces'] for path in piece['files']))
        kept = {key: value for key, value in reply.items()
                if key in {'files', 'forecast', 'violations', 'evidence_contract',
                           'evidence_errors', 'protected_paths', 'memory_ids', 'memory_flags'}}
        reply.clear()
        reply.update(kept, kind='cant_meet', files=kept.get('files', []),
                     report='\n'.join(dict.fromkeys(errors)))
