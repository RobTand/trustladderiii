"""Hosted CI planning and result aggregation. Development tests use PrismaBuild."""


def plan(total):
    raise NotImplementedError


def aggregate(results):
    raise NotImplementedError


def python_partition(files, total):
    raise NotImplementedError
