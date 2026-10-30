"""Evaluate the existing comment classifier without changing its prompt."""


def load_cases(path=None):
    raise NotImplementedError('ticket #52 evaluator')


def request_from_config(path):
    raise NotImplementedError('ticket #52 evaluator')


def evaluate(cases, request, *, model_factory=None):
    raise NotImplementedError('ticket #52 evaluator')


def main(argv=None):
    raise NotImplementedError('ticket #52 evaluator')
