"""Admitted fake-host boundary running the production scratch tree operation."""


def fixture_scratch(facts):
    import json
    import pathlib
    import sys
    import time
    from types import SimpleNamespace
    import os
    output=pathlib.Path('/mnt/shared/flux-out/tl3')/facts['owner']['correlation']
    fixture=output.parent/facts['owner']['correlation'].removesuffix('-out')/'action-fixture.json'
    if fixture.exists():
        settings=json.loads(fixture.read_text())
        sys.path.insert(0,str(pathlib.Path(settings['source'])/'harness/src'))
        from tl3_harness import runtime
        runtime.os.uname=lambda: SimpleNamespace(nodename='dl380g10')
        generation=facts['run']['instance'].split('/')[-1]
        runtime._external_generation=lambda *args: {'exit_code':0,'stdout':generation,'stderr':''}
        if (output/'scratch-pause').exists():
            with (output/'scratch-entered').open('x') as marker:
                marker.write('admitted fixture helper reached before tree operation\n')
                marker.flush()
                os.fsync(marker.fileno())
            until=time.monotonic()+10
            while not (output/'scratch-release').exists():
                if time.monotonic()>=until: raise RuntimeError('fixture pause timed out')
                time.sleep(.01)
        result=runtime._scratch_cleanup_bound(facts, ['fixture',generation], pathlib.Path(json.loads((pathlib.Path('/mnt/shared/flux-out/tl3')/facts['owner']['correlation']/'request.json').read_text())['scratch_root']), systemctl='/unused')
    else:
        now=time.monotonic_ns()
        guard={'exit_code':0,'stdout':facts['run']['instance'].split('/')[-1],'stderr':''}
        result={key:facts[key] for key in ('run','owner','request_digest','host','nonce')}
        result.update(schema='tl3.scratch_cleanup.v1',path=str(pathlib.Path(json.loads((pathlib.Path('/mnt/shared/flux-out/tl3')/facts['owner']['correlation']/'request.json').read_text())['scratch_root'])/facts['run']['job']),uid=1000,
          generation_before=guard,generation_after=guard,started_monotonic_ns=now,finished_monotonic_ns=now+1,
          observation={'kind':'absent','errno':2},attribution=None,manifest=None)
    control=output/'scratch-control.json'
    if control.exists(): result.update(json.loads(control.read_text()).get('replace',{}))
    print(json.dumps(result),flush=True)
