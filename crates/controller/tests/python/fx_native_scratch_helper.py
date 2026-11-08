#!/usr/bin/python3
"""Deterministic absent-root boundary for Native CLI unit fixtures, without host probes."""
import json
import pathlib
import sys
import time
facts=json.load(sys.stdin)
assert facts['schema']=='tl3.scratch_cleanup_input.v1'
now=time.monotonic_ns()
guard={'exit_code':0,'stdout':facts['run']['instance'].split('/')[-1],'stderr':''}
result={key:facts[key] for key in ('run','owner','request_digest','host','nonce')}
result.update(schema='tl3.scratch_cleanup.v1',path=str(pathlib.Path(json.loads((pathlib.Path('/mnt/shared/flux-out/tl3')/facts['owner']['correlation']/'request.json').read_text())['scratch_root'])/facts['run']['job']),uid=1000,
    generation_before=guard,generation_after=guard,started_monotonic_ns=now,finished_monotonic_ns=now+1,
    observation={'kind':'absent','errno':2},attribution=None,manifest=None)
print(json.dumps(result))
