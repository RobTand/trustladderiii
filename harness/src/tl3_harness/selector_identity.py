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


def selection_matches(value, selectors):
    """Compare a full list or digest to the trusted full selection, never its prefix."""
    if isinstance(value, list):
        return value == list(selectors)
    return (isinstance(value, dict) and type(value.get('count')) is int
            and value == selector_identity(selectors))
