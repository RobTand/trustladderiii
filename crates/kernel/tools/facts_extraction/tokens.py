"""Read-only token evidence for the facts standing-rule extraction."""
from pathlib import Path
import hashlib
import json
import re


def block(text, start):
    depth = 0
    for end in range(start, len(text)):
        if text[end] == '{':
            depth += 1
        elif text[end] == '}':
            depth -= 1
            if depth == 0:
                return text[start + 1:end], end + 1
    raise ValueError('unbalanced block')


def executable(text):
    for macro in ('kernel_snapshot!', 'kernel_proof!'):
        while macro in text:
            start = text.index(macro)
            _, end = block(text, text.index('{', start))
            text = text[:start] + text[end:]
    return re.sub(r'//[^\n]*', '', text)


def tokens(text):
    return re.findall(r'\w+|[^\s]', executable(text))


source = Path('crates/kernel/src/facts.rs').read_text()
reference = Path('crates/kernel/src/facts_equivalence/reference.rs').read_text()
rows = []
for arm, helper in (
    ('PrClosedByPerson', 'fact_pr_closed'),
    ('PrDequeuedByPerson', 'fact_pr_dequeued'),
    ('PushByOther { head }', 'fact_push_by_other'),
    ('CiFailed { infra }', 'fact_ci_failed'),
    ('UpstreamStalled', 'fact_upstream_stalled'),
    ('IncidentCleared { by_rob }', 'fact_incident_cleared'),
):
    start = reference.index('Fact::' + arm + ' => ') + len('Fact::' + arm + ' => ')
    if reference[start] == '{':
        old, _ = block(reference, start)
    else:
        old = reference[start:reference.index('),', start) + 1] + ';'
    sig = source.index('fn ' + helper + '(')
    new, _ = block(source, source.index('] {', sig) + 2)
    assert tokens(old) == tokens(new), helper
    rows.append({'helper': helper, 'runtime_tokens': len(tokens(new)),
                 'sha256': hashlib.sha256(' '.join(tokens(new)).encode()).hexdigest(),
                 'token_diff': []})
ci = reference[reference.index('pub(super) fn ci_failed'):]
start = ci.index('    if !infra {')
old, end = block(ci, ci.index('{', start))
for helper, old in [('ci_failed_round', old), ('ci_failed_infrastructure', ci[end:ci.rfind('}')])]:
    sig = source.index('fn ' + helper + '(')
    new, _ = block(source, source.index('] {', sig) + 2)
    assert tokens(old) == tokens(new), helper
    rows.append({'helper': helper, 'runtime_tokens': len(tokens(new)),
                 'sha256': hashlib.sha256(' '.join(tokens(new)).encode()).hexdigest(),
                 'token_diff': []})
print(json.dumps({'classification': 'behaviour-identical (standing rule)', 'moves': rows}, indent=2))
