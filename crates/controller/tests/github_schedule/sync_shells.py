"""Keep executable shell fixtures byte-for-byte equal to their source functions.

Python is repository test glue only. Decisions execute in the copied Rust bodies.
Run with --freeze once at the base; ordinary runs update only the production copy.
"""
import argparse
import pathlib
import subprocess

BASE = "cdacce8f99c28ca8dba8f5f76831b77b4dfcd836"
HERE = pathlib.Path(__file__).resolve().parent
ROOT = HERE.parents[3]
SOURCE = "crates/controller/src/adapters/github.rs"
METHODS = ["request_notice", "close_reason", "reconcile", "execute_delivery", "recover_deliveries"]
HELPERS = ["fence", "client", "linked", "file_request", "console_note", "restart"]
FUNCTIONS = ["writer_input", "writer_facts"]


def function(source, name, indent, occurrence=0):
    prefix = " " * indent
    import re
    starts = list(re.finditer(r"^" + prefix + r"(?:pub(?:\(crate\))? )?fn " + name + r"\(", source, re.M))
    start = starts[occurrence].start()
    end = source.index("\n" + prefix + "}", start) + len(prefix) + 2
    return source[start:end]


def shell(source, old):
    # The first reconcile belongs to Fake. Select the Adapter's second body.
    methods = [function(source, name, 4, 1 if name == "reconcile" else 0) for name in METHODS + HELPERS]
    functions = [function(source, name, 0) for name in FUNCTIONS if "fn " + name + "(" in source]
    imports = "use super::fixture::*;\n"
    if not old:
        imports += "use super::decide;\n"
        if "fn writer_facts(" not in source:
            imports += "use super::decide::writer_facts;\n"
    wrappers = """
pub fn reconcile(shell:&mut Adapter,delivery:&Delivery)->Result<Reconciliation> {shell.reconcile(delivery)}
pub fn execute(shell:&mut Adapter,delivery:&Delivery)->Result<Completion> {shell.execute_delivery(delivery)}
pub fn recover(shell:&mut Adapter)->Result<Option<u64>> {shell.recover_deliveries(4,0)}
"""
    return imports + "type Adapter = Shell<" + str(old).lower() + ">;\nimpl Adapter {\n" + "\n".join(methods) + "\n}\n" + "\n".join(functions) + "\n" + wrappers


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--freeze", action="store_true")
    args = parser.parse_args()
    if args.freeze:
        base = subprocess.check_output(["git", "show", BASE + ":" + SOURCE], cwd=ROOT, text=True)
        (HERE / "legacy_controller.rs").write_text(shell(base, True))
    (HERE / "production_controller.rs").write_text(shell((ROOT / SOURCE).read_text(), False))


if __name__ == "__main__":
    main()
