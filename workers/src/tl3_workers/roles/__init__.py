"""Register each role module here, once."""

from importlib import import_module


ROLE_MODULES = (
    "ingest",
    "audit",
)
ROLES = {name: import_module(f"{__name__}.{name}").ROLE for name in ROLE_MODULES}
