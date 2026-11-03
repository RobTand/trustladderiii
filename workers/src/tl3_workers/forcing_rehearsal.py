"""Trusted, bounded plan-time evidence for Q55 forcings."""
from copy import deepcopy
from pathlib import Path

from tl3_harness import Check, Forcing, Kind
from tl3_harness.node_ids import selects
from tl3_workers.pb_tests import TestSettings, make_test_tool


MAX_REHEARSALS = 8
REHEARSAL_SECONDS = 120
GUIDANCE = (
    'Before a Q55 leaf, call rehearse_forcing with the exact base selectors and forcing. '
    'Each delayed target needs a positive invocation count on each named old test, '
    'and the forced base must fail by assertion after an unforced base pass. '
    f'At most {MAX_REHEARSALS} rehearsals run, each with at most two PB-admitted actions '
    f'and a {REHEARSAL_SECONDS}-second execution limit per action. '
    'A failed, stopped or incomplete rehearsal ends planning. Return cant_meet; never retry '
    'the experiment or route it into reply correction. Code binds verified receipts to '
    'the immutable base, selectors and exact forcing. Independent Checking remains required.'
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


class ForcingRehearsals:
    def __init__(self, worktree, inputs, *, checkout=None):
        self.worktree = worktree
        self.base_sha = inputs.get('base_sha')
        self.failure = None
        self.receipts = []
        self.attempts = 0
        worktree.test_checkout = Path.cwd() if checkout is None else Path(checkout)
        worktree.check_base_sha = self.base_sha
        worktree.test_settings = TestSettings(**inputs['tests']) if 'tests' in inputs else None

    def run(self, tests: list[str], forcing: dict) -> dict:
        """Rehearse exact old tests on the immutable base, once, with a declarative forcing."""
        parsed = Forcing.parse(forcing)
        if not isinstance(tests, list) or not tests or len(tests) > 256:
            raise ValueError('A rehearsal needs 1 to 256 exact base test selectors')
        selected = Check('rehearsal', Kind.DEMONSTRATING, tuple(tests)).tests
        wanted = identity(self.base_sha, selected, parsed)
        if self.failure:
            return deepcopy(self.failure)
        cached = next((receipt for receipt in self.receipts
                       if all(receipt[key] == value for key, value in wanted.items())), None)
        if cached:
            return deepcopy(cached)
        result = {**wanted, 'pending_checking': True}

        def refuse(reason):
            result.update(status='refused', message='Forcing rehearsal refused: ' + reason)
            self.failure = deepcopy(result)
            return result

        if self.attempts >= MAX_REHEARSALS:
            return refuse(f'bounded action count exhausted ({MAX_REHEARSALS} rehearsals)')
        self.attempts += 1
        baseline = make_test_tool(self.worktree, timeout_seconds=REHEARSAL_SECONDS)(list(selected), base=True)
        result['unforced'] = baseline
        problem = unusable(baseline, selected, 'passed')
        if problem:
            return refuse('unforced base: ' + problem)
        forced = make_test_tool(self.worktree, forcing=parsed, timeout_seconds=REHEARSAL_SECONDS)(list(selected), base=True)
        result['forced'] = forced
        # Diagnose reachability separately from an existing node or a forced base pass.
        delayed = {target for target, mode, _ in parsed.patches if mode == 'delay_seconds'}
        if forced.get('status') == 'completed':
            for item in forced['tests']:
                for target in sorted(delayed):
                    count = item.get('target_hits', {}).get(target)
                    if type(count) is not int or count < 0:
                        return refuse(f"missing invocation count for {target} on {item['test']}")
                    if count == 0:
                        return refuse(f"zero-hit delayed target {target} on {item['test']}")
        problem = unusable(forced, selected, 'failed')
        if problem:
            return refuse('forced base must fail by assertion: ' + problem)
        result['status'] = 'validated'
        self.receipts.append(deepcopy(result))
        return result

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
        return ledger.run(tests, forcing)
    return rehearse_forcing
