"""Discover role modules without a shared registration list."""

from importlib import import_module
from pkgutil import iter_modules


ROLES = {
    module.name: import_module(f"{__name__}.{module.name}").ROLE
    for module in sorted(iter_modules(__path__), key=lambda module: module.name)
    if module.name != "base" and not module.name.startswith("_")
}
