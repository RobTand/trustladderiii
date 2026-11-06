"""Replay sanitized #3312 and #1637 source/brief pairs through real role hooks."""
import copy
import json
import sys

from tl3_workers.roles.ingest import ROLE as INGEST
from tl3_workers.roles.audit import ROLE as AUDIT
from tl3_workers.runtime import initial_message


for case in json.load(sys.stdin):
    inputs = {'issue': case['issue']}
    brief = copy.deepcopy(case['reply'])
    INGEST.check_reply(inputs, brief)
    for role in ('planner', 'maker'):
        downstream = {'ingest': brief} if role == 'planner' else {'brief': brief['brief']}
        message = initial_message({'role': role, 'model': {'name': 'fixture'}}, downstream)
        assert case['node_id'] in message.text, f"{role} lost supplied node ID: {case['node_id']}"
        code = inputs['issue']['body'].split('```python\n', 1)[1].split('```', 1)[0]
        assert code in brief['brief'], f"{role} lost verbatim source test"
    approved = {'verdict': 'agree', 'objections': []}
    AUDIT.check_reply({**inputs, 'ingest': brief}, approved)
    assert approved['verdict'] == 'agree'
    omitted = {'verdict': 'agree', 'objections': []}
    AUDIT.check_reply({**inputs, 'ingest': case['reply']}, omitted)
    assert omitted['verdict'] == 'disagree', 'audit approved the lossy recorded brief'
    assert any('verbatim' in objection for objection in omitted['objections'])
