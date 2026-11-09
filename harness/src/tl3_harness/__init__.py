__all__ = ["Check", "Container", "Custody", "Forcing", "Proof", "HarnessInputError", "ProofRefusal", "Kind", "Result", "Verdict",
           "run_checks"]


def __getattr__(name):
    """Load job-safe exports without executing harness imports for package discovery."""
    if name not in __all__:
        raise AttributeError(name)
    from importlib import import_module
    module = '.execution' if name in ('Container', 'run_checks') else '.model'
    value = getattr(import_module(module, __name__), name)
    globals()[name] = value
    return value
