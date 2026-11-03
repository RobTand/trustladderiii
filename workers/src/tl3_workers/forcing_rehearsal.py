"""Trusted, bounded plan-time evidence for Q55 forcings."""


class ForcingRehearsals:
    def __init__(self, worktree, inputs):
        self.worktree = worktree
        self.inputs = inputs
        self.failure = None
        self.receipts = []

    def run(self, tests: list[str], forcing: dict) -> dict:
        raise NotImplementedError('plan-time forcing rehearsal')

    def violations(self, base_sha, checks):
        raise NotImplementedError('matching forcing rehearsal receipts')
