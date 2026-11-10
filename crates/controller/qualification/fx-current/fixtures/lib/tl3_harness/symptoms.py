"""Exact symptom classification shared by rehearsal and independent Checking."""
import hashlib
import json

from .model import Outcome, Result, TestResult, Verdict


def classify(result, declaration):
    if result.get('status') != 'completed' or result.get('execution'):
        return 'unavailable'
    nodes = result.get('tests', [])
    if len(nodes) != 1 or nodes[0]['test'] != declaration['node']:
        return 'unavailable'
    node = nodes[0]
    if node['outcome'] == 'passed':
        return 'passed'
    failures = node.get('failures', [])
    if (node['outcome'] == 'error' and len(failures) == 1
            and failures[0]['exception'] == declaration['exception']
            and failures[0]['phase'] == declaration['phase'] and failures[0]['source']):
        return 'symptom'
    return 'unrelated'


def receipt(records, result, declaration, number):
    encoded = json.dumps(records, sort_keys=True, separators=(',', ':'), ensure_ascii=False).encode()
    return {'run': number, 'outcome': classify(result, declaration),
            'sha256': hashlib.sha256(encoded).hexdigest(), 'records': records,
            'target_hits': next((record.get('target_hits', {}) for record in records
                                 if record.get('event') == 'result'), {})}


def campaign_outcome(result):
    runs = result.get('symptom_runs', [])
    if not runs or any(run['outcome'] == 'unavailable' for run in runs):
        return Outcome.NO_REPORT
    if any(run['outcome'] == 'unrelated' for run in runs):
        return Outcome.ERROR
    return Outcome.ERROR if any(run['outcome'] == 'symptom' for run in runs) else Outcome.PASSED


def judge_symptom(check, base, head, ordinary, base_sha, head_sha):
    declaration = check.declared_symptom
    campaigns = [('base', base), ('head', head)]
    if check.forcing:
        campaigns.append(('ordinary-head', ordinary))
    runs = [{**run, 'side': side} for side, result in campaigns for run in result['symptom_runs']]
    occurrences = sum(run['outcome'] == 'symptom' for run in base['symptom_runs'])
    complete = all(len(result['symptom_runs']) == declaration['repeat'] for result in (base, head, ordinary))
    red = complete and occurrences > 0 and all(run['outcome'] in ('passed', 'symptom') for run in base['symptom_runs'])
    green = all(run['outcome'] == 'passed' for result in (head, ordinary) for run in result['symptom_runs'])
    passed = red and green
    # Reports are retained in the trusted result, inside the PB action's CAS receipt.
    evidence = {'base': base_sha, 'head': head_sha, 'forcing': check.forcing.as_json() if check.forcing else None,
                'declaration': declaration, 'runs': runs}
    message = '' if passed else f'Declared symptom observed {occurrences} of {declaration["repeat"]} base runs ({len(base['symptom_runs'])} attempted). '
    if not passed:
        message += ('Zero occurrences is not RED. ' if occurrences == 0 else '')
        message += 'Every base failure must match the declaration; every head run must pass under identical settings.'
    before, after = campaign_outcome(base), campaign_outcome(ordinary)
    during = campaign_outcome(head) if check.forcing else None
    unavailable = any(run['outcome'] == 'unavailable' for run in runs)
    return Result(check.name, check.kind, Verdict.PASS if passed else (
        Verdict.INFRASTRUCTURE_FAILURE if unavailable else Verdict.FAIL),
        (TestResult(declaration['node'], before, after, False, passed, message, during),),
        forcing=check.forcing, declared_symptom=declaration, symptom_evidence=evidence)
