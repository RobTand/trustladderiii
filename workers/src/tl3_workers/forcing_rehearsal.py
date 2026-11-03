"""Trusted, bounded plan-time evidence for Q55 forcings."""
from copy import deepcopy
import ast
import json
from pathlib import Path
import re

from tl3_harness import Check, Forcing, Kind
from tl3_harness.node_ids import selects
from tl3_harness.forcing_ownership import ForcingError, preflight_owner
from tl3_workers.pb_tests import TestSettings, make_test_tool, _git
from tl3_workers.session import save


MAX_REHEARSALS = 8
REHEARSAL_SECONDS = 120
GUIDANCE = (
    'Before a Q55 leaf, call rehearse_forcing with the exact base selectors and forcing. '
    'Each delayed target needs a positive invocation count on each named old test, '
    'and the forced base must fail by assertion after an unforced base pass. '
    f'At most {MAX_REHEARSALS} distinct candidates run, each with at most two PB-admitted actions '
    f'and a {REHEARSAL_SECONDS}-second execution limit per action. '
    'A completed zero-hit or no-assertion-RED candidate is unusable evidence. Use its feedback '
    'for a distinct candidate within the remaining budget, or return cant_meet with the precise '
    'capability limit. Identical candidates reuse their positive or negative result. '
    'Infrastructure failures exit the worker for controller-owned recovery, never model repair. '
    'After budget exhaustion return cant_meet once with the recorded evidence. No final cant_meet '
    'is retried. Code binds verified receipts to the immutable base, selectors and exact forcing. '
    'Eligibility uses the resolved module or class, never a repository-looking alias prefix. '
    'Static preflight imports no served code; unresolved dynamic owners are an explicit capability limit. '
    'Independent Checking remains required.'
)


def identity(base_sha, selectors, forcing):
    return {'base_sha': base_sha, 'selectors': list(selectors), 'forcing': forcing.as_json()}


def unusable(result, selectors, outcome):
    if result.get('status') != 'completed' or not result.get('receipts'):
        return (f"missing completed, verified receipt: status={result.get('status', 'unknown')}; "
                f"{result.get('message', 'no diagnostic')}")
    if len(result['tests']) != sum(result['totals'].values()):
        return 'incomplete node results'
    for selector in selectors:
        nodes = [item for item in result['tests'] if selects(selector, item['test'])]
        if not nodes:
            return f'missing node result for {selector}'
        for node in nodes:
            if node['outcome'] != outcome:
                return f"{node['test']} ended {node['outcome']}; expected {outcome}"
    return None


class ForcingInfrastructureError(Exception):
    """Leave the tool/model loop through the existing WorkerFailed boundary."""

    def __init__(self, result):
        self.result = deepcopy(result)
        stage = result['stage']
        evidence = result[stage]
        execution = evidence.get('execution', {})
        # Only identifiers from trusted boundary diagnostics go into controller incidents.
        def token(value):
            return value if isinstance(value, str) and re.fullmatch(r'[a-z0-9_]{1,64}', value) else 'unknown'
        key = evidence.get('action_key')
        key = key if isinstance(key, str) and re.fullmatch(r'[a-f0-9]{64}', key) else 'not_admitted'
        super().__init__(f"Forcing rehearsal {stage}: action={key}; "
                         f"phase={token(execution.get('phase'))}; "
                         f"outcome={token(execution.get('outcome'))}; "
                         f"cause={token(execution.get('termination_reason'))}")


class ForcingRehearsals:
    def __init__(self, worktree, inputs, *, checkout=None, checkpoint=None):
        self.worktree = worktree
        self.base_sha = inputs.get('base_sha')
        self.checkpoint = Path(checkpoint) if checkpoint else None
        self.contract = {'base_sha': self.base_sha, 'tests': inputs.get('tests')}
        self.failure = self.infrastructure = self.active = None
        self.receipts = []
        self.candidates = []
        self.attempts = self.actions = 0
        self.sources = {}
        self.entries = None
        if self.checkpoint and self.checkpoint.exists():
            state = json.loads(self.checkpoint.read_text())
            if state['version'] != 1 or state['contract'] != self.contract:
                raise ValueError('forcing rehearsal checkpoint contract changed')
            self.candidates = state['candidates']
            self.receipts = [deepcopy(item) for item in self.candidates if item['status'] == 'validated']
            self.attempts, self.actions = state['attempts'], state['actions']
            self.active, self.failure = state['active'], state['failure']
        worktree.test_checkout = Path.cwd() if checkout is None else Path(checkout)
        worktree.check_base_sha = self.base_sha
        worktree.test_settings = TestSettings(**inputs['tests']) if 'tests' in inputs else None
        worktree.pb_strict_reattach = True
        worktree.pb_submitted = self.submitted

    def persist(self):
        if self.checkpoint:
            save(self.checkpoint, {'version': 1, 'contract': self.contract, 'candidates': self.candidates,
                                   'attempts': self.attempts, 'actions': self.actions,
                                   'active': self.active, 'failure': self.failure})

    def submitted(self, key, base, tree):
        self.actions += 1
        self.active['inflight'] = {'key': key, 'tree': tree, 'identity': self.worktree.pb_identity}
        self.persist()

    def source(self, module):
        if module in self.sources:
            return self.sources[module]
        if self.entries is None:
            self.entries = {}
            for entry in _git(self.worktree.test_checkout, 'ls-tree', '-rz', self.base_sha).split('\x00'):
                if entry:
                    metadata, path = entry.split('\t', 1)
                    mode, kind, oid = metadata.split()
                    if mode in {'100644', '100755'} and kind == 'blob':
                        self.entries[path] = oid
        path = module.replace('.', '/')
        candidates = [(prefix + path + suffix, suffix == '/__init__.py')
                      for prefix in ('', 'src/') for suffix in ('.py', '/__init__.py')
                      if prefix + path + suffix in self.entries]
        if not candidates:
            self.sources[module] = None
            return None
        if len(candidates) != 1:
            raise ForcingError(target=module, module=module, category='unresolved', reason='ambiguous_module_source')
        filename, package = candidates[0]
        oid = self.entries[filename]
        if int(_git(self.worktree.test_checkout, 'cat-file', '-s', oid)) > 1_048_576:
            raise ForcingError(target=module, module=module, category='unresolved', reason='source_exceeds_limit')
        try:
            tree = ast.parse(_git(self.worktree.test_checkout, 'cat-file', 'blob', oid))
        except (SyntaxError, UnicodeError) as error:
            raise ForcingError(target=module, module=module, category='unresolved', reason='invalid_module_source') from error
        self.sources[module] = tree, package
        return self.sources[module]

    def raise_infrastructure(self):
        if self.infrastructure:
            raise ForcingInfrastructureError(self.infrastructure)

    def run(self, tests: list[str], forcing: dict) -> dict:
        """Rehearse immutable old tests and cache each distinct candidate's outcome."""
        parsed = Forcing.parse(forcing)
        if not isinstance(tests, list) or not tests or len(tests) > 256:
            raise ValueError('A rehearsal needs 1 to 256 exact base test selectors')
        selected = Check('rehearsal', Kind.DEMONSTRATING, tuple(tests)).tests
        wanted = identity(self.base_sha, selected, parsed)
        if self.infrastructure or self.failure:
            return deepcopy(self.infrastructure or self.failure)
        cached = next((candidate for candidate in self.candidates
                       if all(candidate[key] == value for key, value in wanted.items())), None)
        if cached:
            return deepcopy(cached)
        if self.active is not None:
            if any(self.active['result'][key] != value for key, value in wanted.items()):
                # Recovery must settle the known action before exploring another proposal.
                pending = self.active['result']
                stage = self.active['stage']
                evidence = {'message': 'Resume the exact pending rehearsal candidate before a distinct proposal.',
                            'execution': {'phase': 'reattachment', 'outcome': 'transport_failure',
                                          'termination_reason': 'pending_candidate_mismatch'}}
                if self.active['inflight']:
                    evidence['action_key'] = self.active['inflight']['key']
                self.infrastructure = {**deepcopy(pending), stage: evidence, 'stage': stage,
                                       'status': 'infrastructure_error', 'message': evidence['message']}
                return deepcopy(self.infrastructure)
            result = self.active['result']
        else:
            result = {**wanted, 'pending_checking': True}
            if self.attempts >= MAX_REHEARSALS:
                result.update(status='exhausted', message=(
                    f'Forcing rehearsal budget exhausted ({MAX_REHEARSALS} rehearsals, '
                    f'{self.actions} admitted actions, at most 16). Return cant_meet with this evidence.'))
                self.failure = deepcopy(result)
                self.persist()
                return result
            self.attempts += 1
            self.active = {'result': result, 'stage': 'unforced', 'inflight': None}
            self.persist()

        def refuse(reason, *, terminal=False):
            result.update(status='refused', message='Forcing rehearsal refused: ' + reason)
            self.candidates.append(deepcopy(result))
            self.active = None
            if terminal:
                self.failure = deepcopy(result)
            self.persist()
            return deepcopy(result)

        try:
            for target, _, _ in parsed.patches:
                preflight_owner(target, self.source)
        except ForcingError as error:
            result['refusal'] = error.refusal
            return refuse(str(error))

        for stage, expected in (('unforced', 'passed'), ('forced', 'failed')):
            if stage not in result:
                self.active['stage'] = stage
                inflight = self.active['inflight']
                self.worktree.pb_reattach = {True: inflight} if inflight else {}
                tool = make_test_tool(self.worktree, timeout_seconds=REHEARSAL_SECONDS,
                                      **({'forcing': parsed} if stage == 'forced' else {}))
                observed = tool(list(selected), base=True)
                if observed['status'] == 'infrastructure_error':
                    self.infrastructure = {**deepcopy(result), stage: observed, 'stage': stage,
                                           'status': 'infrastructure_error', 'message': observed['message']}
                    self.active['infrastructure'] = deepcopy(self.infrastructure)
                    self.persist()
                    return deepcopy(self.infrastructure)
                result[stage] = observed
                self.active['inflight'] = None
                self.persist()
            observed = result[stage]
            if stage == 'forced' and observed['status'] == 'completed':
                delayed = {target for target, mode, _ in parsed.patches if mode == 'delay_seconds'}
                for item in observed['tests']:
                    if item.get('forcing_refusal'):
                        result['refusal'] = deepcopy(item['forcing_refusal'])
                        refusal = result['refusal']
                        return refuse(f"forcing validation for {refusal['target']}: resolved module "
                                      f"{refusal['resolved_module']}, {refusal['owner_category']}, {refusal['reason']}")
                    for target in sorted(delayed):
                        count = item.get('target_hits', {}).get(target)
                        if type(count) is not int or count < 0:
                            return refuse(f"missing invocation count for {target} on {item['test']}")
                        if count == 0:
                            return refuse(f"zero-hit delayed target {target} on {item['test']}")
            problem = unusable(observed, selected, expected)
            if problem:
                prefix = 'unforced base: ' if stage == 'unforced' else 'forced base must fail by assertion: '
                return refuse(prefix + problem, terminal=observed['status'] != 'completed')
        result['status'] = 'validated'
        self.receipts.append(deepcopy(result))
        self.candidates.append(deepcopy(result))
        self.active = None
        self.persist()
        return deepcopy(result)

    def violations(self, base_sha, checks):
        if self.failure:
            return [self.failure['message']]
        errors = []
        for check in checks:
            if 'forcing' not in check:
                continue
            wanted = identity(base_sha, check['node_ids'], Forcing.parse(check['forcing']))
            if not any(all(receipt[key] == value for key, value in wanted.items()) for receipt in self.receipts):
                errors.append('Q55 check lacks a matching forcing rehearsal receipt for the exact base, '
                              'selectors and forcing: ' + ', '.join(check['node_ids']))
        return errors


def make_rehearsal_tool(worktree):
    def rehearse_forcing(tests: list[str], forcing: dict) -> dict:
        """Rehearse exact old tests on the immutable base with a declarative forcing."""
        ledger = getattr(worktree, 'forcing_rehearsals', None)
        if ledger is None:
            return {'status': 'refused', 'message': 'Forcing rehearsal is unavailable for this role.'}
        result = ledger.run(tests, forcing)
        ledger.raise_infrastructure()
        return result
    return rehearse_forcing
