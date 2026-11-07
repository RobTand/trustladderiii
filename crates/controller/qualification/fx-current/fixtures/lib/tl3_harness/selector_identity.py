"""Bounded identities for exact, ordered pytest selections."""
import hashlib
import json


def selector_identity(selectors):
    # Preserve order and duplicates: receipt comparison historically used list equality.
    selected = list(selectors)
    raw = json.dumps(selected, ensure_ascii=True, separators=(',', ':')).encode()
    return {'schema': 'tl3.selectors.v1', 'count': len(selected),
            'sha256': hashlib.sha256(raw).hexdigest(),
            'prefix': [test[:120] for test in selected[:3]]}
