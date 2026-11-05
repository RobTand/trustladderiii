"""Seed one production-body fault without changing its Verus contract.

Run in a scratch branch, then run the named Verus group. The differential also
rejects acknowledged, identity_bits, run_succeeded and delivery-policy faults.
"""
import argparse
from pathlib import Path

MUTANTS = {
    "run_succeeded": ("qualification", "false"),
    "review_eligible": ("qualification", "false"),
    "head_eligible": ("qualification", "false"),
    "record_fits": ("bounds", "bytes <= 32769"),
    "stored_record_fits": ("bounds", "bytes <= 32769"),
    "acknowledged": ("completion", "success && asynchronous && same_bank && one_item"),
    "retain_action": ("completion", "Action::Done"),
    "made_progress": ("completion", "true"),
    "http_action": ("classification", "Action::Done"),
    "identity_bits": ("identity", "(0, 0)"),
}


def mutated(text, name):
    start = text.index("{", text.index("pub fn " + name + "("))
    end, depth = start + 1, 1
    while depth:
        depth += (text[end] == "{") - (text[end] == "}")
        end += 1
    return text[:start + 1] + "\n        " + MUTANTS[name][1] + "\n    " + text[end - 1:]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("function", choices=MUTANTS)
    args = parser.parse_args()
    path = Path(__file__).resolve().parent.parent / "src/memory_retain.rs"
    path.write_text(mutated(path.read_text(), args.function))
    print("//crates/controller-core:verus_memory_retain_" + MUTANTS[args.function][0])


if __name__ == "__main__":
    main()
