"""Run Cargo test binaries concurrently and enforce per-test wall time budgets."""


def check_budget(results, config, baseline):
    raise NotImplementedError


def execute_binary(binary, custom, arguments=()):
    raise NotImplementedError


def execute_binaries(binaries, jobs):
    raise NotImplementedError
