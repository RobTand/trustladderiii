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


def preflight(inputs, reply, *, planning=False):
    # Old in-flight requests have no capability declaration (#372).
    if 'tool_capabilities' not in inputs:
        return
    if 'evidence_contract' not in reply:
        raise ValueError('derive evidence_contract from every mandatory execution requirement')
    if planning and any('helper_evidence' in check for check in reply.get('checks', [])):
        reply['evidence_contract'].append({
            'requirement': 'Observe the real old and new test helper with identical new regression assertions.',
            'tool': 'run_tests', 'executor': 'prismabuild_pytest', 'runner_features': ['base_head'],
            'artifacts': ['cas_receipt', 'terminal_verdict'], 'methods': ['helper_target_observer'],
            'protected_files': []})
    contract = []
    for requirements in (inputs.get('ingest', {}).get('evidence_contract', []),
                         inputs.get('audit_evidence_contract', []), reply['evidence_contract']):
        for requirement in requirements:
            if requirement not in contract:
                contract.append(requirement)
    errors = []
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
