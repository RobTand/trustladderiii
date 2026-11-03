"""Compare a request's evidence with the installed tools, without substituting proof."""

DIMENSIONS = ('runner_features', 'artifacts', 'methods', 'protected_files')


def preflight(inputs, reply, *, planning=False):
    # Old in-flight requests have no capability declaration (#372).
    if 'tool_capabilities' not in inputs:
        return
    if 'evidence_contract' not in reply:
        raise ValueError('derive evidence_contract from every mandatory execution requirement')
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
