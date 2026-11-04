"""Generate the test-only o200k_base oracle with the worker's offline loader."""
import argparse
import ast
import hashlib
import importlib.metadata
import json
from pathlib import Path
import random
import statistics
import sys
import time
import types


ROOT = Path(__file__).resolve().parents[3]
OUTPUT = ROOT / "crates/config/tests/tokenizer/oracle.json"


def encoding():
    if importlib.metadata.version("tiktoken") != "0.14.0":
        raise RuntimeError("the oracle requires tiktoken 0.14.0")
    path = ROOT / "workers/src/tl3_workers/forecast.py"
    source = ast.parse(path.read_text())
    # Use the actual offline worker loader without importing its agent stack.
    selected = [node for node in source.body if (
        isinstance(node, (ast.Import, ast.ImportFrom))
        and all(alias.name in {"base64", "hashlib", "os", "tiktoken", "Path", "PurePosixPath"}
                for alias in node.names)
    ) or (isinstance(node, ast.Assign) and any(
        isinstance(target, ast.Name) and target.id.startswith("TOKENIZER_") for target in node.targets
    )) or (isinstance(node, ast.FunctionDef) and node.name == "optional_encoding")]
    module = types.ModuleType("offline_forecast")
    exec(compile(ast.Module(body=selected, type_ignores=[]), str(path), "exec"), module.__dict__)
    result = module.optional_encoding()
    if result is None:
        raise RuntimeError("preload the pinned file in TIKTOKEN_CACHE_DIR; no downloads are allowed")
    return result, module.TOKENIZER_SHA256


def fixture(name):
    path = ROOT / "workers/tests/test_large_files.py"
    for node in ast.parse(path.read_text()).body:
        if isinstance(node, ast.Assign) and any(isinstance(t, ast.Name) and t.id == name for t in node.targets):
            return eval(compile(ast.Expression(node.value), str(path), "eval")).decode()
    raise RuntimeError(f"fixture {name} is missing")


def corpus():
    for name, text in {
        "empty": "", "rust": "fn main() { let café = vec![1_u64, 2]; println!(\"{:?}\", café); }\n",
        "python": "async def café(x: str):\n    return f'{x!r}'  # 👩🏽‍💻\n",
        "typescript": "export const f = (x: string): string => `${x?.trim()} 🌍`;\n",
        "go": "package main\nfunc main() { ch := make(chan int); go func(){ ch <- 42 }() }\n",
        "sql": "SELECT id, 'こんにちは' FROM jobs WHERE state != 'done';\n",
        "shell": "#!/bin/sh\nset -eu\nprintf '%s\\n' \"$value\"\n",
        "unicode": "こんにちは 🌍 مرحبا नमस्ते 한국어 Ελληνικά עברית 中文\r\n",
        "normalization": "café cafe\u0301 Å A\u030a İ ı ß ﬁ 가 가\u200d\u200c\ufeff",
        "emoji": "👩🏽‍💻👨‍👩‍👧‍👦🇬🇧🏳️‍🌈🫩🧑🏿‍🚀1️⃣\ufe0f",
        "controls": "\x00\x01\x7f\r\n\r\t\v\f\u0085\u2028\u2029",
        "special_literals": "<|endoftext|> <|endofprompt|> <|fim_prefix|>",
        "contractions": "I'm I'M we'd WE'D couldn't Couldn't it's IT'S you've YOU'VE\n",
        "numbers": "0 12 123 1234 12345678901234567890 １２３４ ١٢٣٤\n",
        "punctuation": "!?!!!...//\r\n</tag>\\\\'\"`~{}[]:=+-_*&^%$#@\n",
        "forecast_loader_fixture": "Hello, WORLD! I'm testing.\r\n",
        "forecast_punctuation_fixture": "123456789\t\n  punctuation!?!",
        "forecast_brief": "<|endoftext|>",
    }.items():
        yield {"group": "curated", "name": name, "text": text}
    for index, text in enumerate(["a", " ", "\t", "\n", "\r\n", "!", "abc123", "👩🏽‍💻", "a\u0301"]):
        for repeat in [2, 3, 7, 31, 257, 1024]:
            yield {"group": "curated", "name": f"bpe-run-{index}-{repeat}", "text": text, "repeat": repeat}
    for extra in [-1, 0, 1]:
        yield {"group": "large", "name": f"large-file-boundary-{extra:+}", "text": "hello world\n",
               "repeat": 6000, "truncate_bytes": 65_536 + extra}
    yield {"group": "large", "name": "forecast-unicode-fixture", "text": "こんにちは 🌍 <|endoftext|>\r\n",
           "repeat": 700, "suffix": "last line"}
    architecture = {"group": "large", "name": "large-files-ARCHITECTURE", "prefix": "# Architecture\n\n",
                    "text": "The quantizer packs weights per channel.\n", "repeat": 62_000}
    assert expanded(architecture) == fixture("ARCHITECTURE")
    yield architecture
    yield {"group": "large", "name": "large-files-POOL", "text": fixture("POOL")}
    yield {"group": "large", "name": "long-line", "text": "abc123!?", "repeat": 8192}
    rng = random.Random(748)
    alphabet = "abcXYZ0123456789 \t\r\n!?/'_-<|>éßİ中あ한اक\u0301\u200d🌍👩🏽"
    for index in range(2048):
        length = rng.randrange(0, 257)
        if index % 2:
            text = "".join(rng.choice(alphabet) for _ in range(length))
        else:
            # Rust strings contain Unicode scalar values, never lone surrogates.
            points = [rng.randrange(0x110000 - 0x800) for _ in range(length)]
            text = "".join(chr(p + 0x800 if p >= 0xD800 else p) for p in points)
        yield {"group": "random", "name": f"seed-748-{index}", "text": text}


def expanded(case):
    text = case["text"] * case.get("repeat", 1)
    if "truncate_bytes" in case:
        text = text.encode()[:case["truncate_bytes"]].decode()
    return case.get("prefix", "") + text + case.get("suffix", "")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=OUTPUT)
    parser.add_argument("--benchmark", action="store_true")
    parser.add_argument("--check", action="store_true", help="check the selected oracle without writing it")
    args = parser.parse_args()
    tokenizer, pin = encoding()
    cases = list(corpus())
    for case in cases:
        text = expanded(case)
        case["bytes"] = len(text.encode())
        case["sha256"] = hashlib.sha256(text.encode()).hexdigest()
        case["tokens"] = len(tokenizer.encode_ordinary(text))
        if args.benchmark and case["name"] in {"large-files-ARCHITECTURE", "large-files-POOL", "large-file-boundary-+0"}:
            durations = []
            for _ in range(5):
                start = time.perf_counter()
                assert len(tokenizer.encode_ordinary(text)) == case["tokens"]
                durations.append(time.perf_counter() - start)
            print(json.dumps({"python_benchmark": case["name"], "bytes": case["bytes"],
                              "median_ms": statistics.median(durations) * 1000, "calls": 5}), flush=True)
    record = {"python_tiktoken": "0.14.0", "bpe_sha256": pin, "pattern": tokenizer._pat_str,
              "random_seed": 748, "cases": cases}
    generated = (json.dumps(record, ensure_ascii=True, indent=2) + "\n").encode()
    if args.check:
        try:
            committed = args.output.read_bytes()
        except FileNotFoundError:
            print(f"oracle missing: {args.output}", file=sys.stderr)
            return 1
        if generated != committed:
            print(f"oracle drift: {args.output}; regenerate with tokenizer_oracle.py", file=sys.stderr)
            return 1
        print(f"checked {len(cases)} cases in {args.output}", flush=True)
    else:
        args.output.write_bytes(generated)
        print(f"wrote {len(cases)} cases to {args.output}", flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
