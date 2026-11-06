"""Strict transport for ingest and audit verbatim material retention."""

def preserve_verbatim(role, inputs, reply):
    from tl3_workers.reply_check import run_check, ReplyViolation, ReplyCheckInfrastructureError
    # Lazy transport dispatch only. Rust parses source material and decides what to retain.
    body = inputs.get('issue', {}).get('body') or ''
    if '```' not in body and '~~~' not in body:
        return
    result = run_check(['check-reply', role], {'inputs': inputs, 'reply': reply})
    if (not isinstance(result, dict) or set(result) != {'reply', 'violations'}
            or not isinstance(result['reply'], dict) or not isinstance(result['violations'], list)
            or any(not isinstance(item, str) for item in result['violations'])):
        raise ReplyCheckInfrastructureError('reply validator returned invalid output')
    if result['violations']:
        raise ReplyViolation(result['violations'][0])
    reply.clear()
    reply.update(result['reply'])


