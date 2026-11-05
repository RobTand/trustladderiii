"""I/O-free transport glue for the release's Rust PB evidence decisions."""
import base64
import json

from tl3_harness.supervision import ExecutionFailure, ExecutionOutcome
from tl3_workers.reply_check import ReplyCheckInfrastructureError, run_check


def encoded(raw):
    return base64.b64encode(raw).decode('ascii')


def _error(value):
    if not isinstance(value, dict) or not isinstance(value.get('message'), str):
        raise ReplyCheckInfrastructureError('PB evidence validator returned invalid output')
    kind = value.get('type')
    if kind == 'ExecutionFailure':
        execution = value.get('execution', {})
        error = ExecutionFailure(ExecutionOutcome(execution['outcome']), execution['phase'],
                                 execution['termination_reason'])
    elif kind == 'JSONDecodeError':
        detail = value['json']
        error = json.JSONDecodeError(detail['reason'], detail['document'], detail['position'])
    elif kind == 'KeyError':
        error = KeyError(value['message'][1:-1])
    elif kind == 'UnicodeDecodeError':
        detail = value['unicode']
        error = UnicodeDecodeError('utf-8', base64.b64decode(detail['raw']), detail['start'], detail['end'], detail['reason'])
    elif kind in ('TypeError', 'AttributeError'):
        error = {'TypeError': TypeError, 'AttributeError': AttributeError}[kind](value['message'])
    elif kind == 'ValueError':
        error = ValueError(value['message'])
    else:
        raise ReplyCheckInfrastructureError('PB evidence validator returned invalid output')
    if value.get('cause') is not None:
        object.__setattr__(error, '__cause__', _error(value['cause']))
    return error


def check(operation, **facts):
    verdict = run_check(['check-pb', operation], facts, name='PB evidence')
    if not isinstance(verdict, dict):
        raise ReplyCheckInfrastructureError('PB evidence validator returned invalid output')
    if set(verdict) == {'return'}:
        return verdict['return']
    if set(verdict) == {'raise'}:
        try:
            error = _error(verdict['raise'])
        except (KeyError, TypeError, ValueError):
            raise ReplyCheckInfrastructureError('PB evidence validator returned invalid output') from None
        raise error
    raise ReplyCheckInfrastructureError('PB evidence validator returned invalid output')


def fields(value, names):
    """Project consumed facts before crossing the wire; omit PB private metadata."""
    return {key: value[key] for key in names if key in value} if isinstance(value, dict) else value


def ending_facts(ending):
    result = fields(ending, ('action_key', 'status', 'exit_code', 'succeeded', 'attempt', 'reason'))
    if not isinstance(result, dict):
        return result
    evidence = fields(ending.get('result'), ('verified', 'receipt_sha256', 'stdout'))
    if isinstance(evidence, dict) and 'stdout' in evidence:
        evidence['stdout'] = fields(evidence['stdout'], ('path', 'bytes', 'sha256'))
    if 'result' in ending:
        result['result'] = evidence
    if 'publication' in ending:
        result['publication'] = fields(ending['publication'], ('published_unix', 'generation'))
    diagnostics = fields(ending.get('diagnostics'), ('returncode', 'termination_reason', 'elapsed_s', 'resource_profile'))
    if isinstance(diagnostics, dict) and 'resource_profile' in diagnostics:
        profile = fields(diagnostics['resource_profile'], ('scope',))
        if isinstance(profile, dict) and 'scope' in profile:
            profile['scope'] = fields(profile['scope'], ('memory_peak_bytes',))
        diagnostics['resource_profile'] = profile
    if 'diagnostics' in ending:
        result['diagnostics'] = diagnostics
    return result
