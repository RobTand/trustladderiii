"""Compare the real prose checker with GitHub rendering. Run through PrismaBuild.

Uses gh api POST /markdown only. It does not publish comments or issues.
"""
import collections
import hashlib
from html.parser import HTMLParser
import json
from pathlib import Path
import subprocess
import tempfile
import urllib.request


class Paragraphs(HTMLParser):
    def __init__(self):
        super().__init__()
        self.rows = []
        self.row = None

    def handle_starttag(self, tag, attrs):
        if tag == "p":
            assert self.row is None
            self.row = {"text": "", "links": []}
        if tag == "a" and self.row is not None:
            classes = dict(attrs).get("class", "").split()
            self.row["links"].extend(
                name for name in classes if name in {"user-mention", "issue-link"}
            )

    def handle_data(self, data):
        if self.row is not None:
            self.row["text"] += data

    def handle_endtag(self, tag):
        if tag == "p":
            assert self.row is not None
            self.rows.append(self.row)
            self.row = None


def main():
    root = Path(__file__).resolve().parents[3]
    # Pin the character database so the sweep includes every Cf in Unicode 17.
    url = "https://www.unicode.org/Public/17.0.0/ucd/UnicodeData.txt"
    with tempfile.TemporaryDirectory(prefix="tl3-unicode-") as directory:
        database = Path(directory) / "UnicodeData.txt"
        urllib.request.urlretrieve(url, database)
        data = database.read_bytes()
    formats = [
        chr(int(fields[0], 16))
        for line in data.decode().splitlines()
        if (fields := line.split(";"))[2] == "Cf"
    ]
    assert len(formats) >= 170, "incomplete Unicode character database"
    samples = []

    def add(group, prefix, suffix=""):
        for token in ["@octocat", "gh-5", "GH-5"]:
            samples.append((group, f"{prefix}{token}{suffix}", False))

    for prefix in ["ｏ", "１", "²", "ⓐ", "𝐚", "a\u200b", "a\u2060", "a\u200d", "a\ufeff"]:
        add("review", prefix)
    add("review", "(ｏ", ")")
    add("review", "_ｏ", "_")
    for prefix in ["а", "ο", "a\u0301"]:
        add("letters-and-marks", prefix)
    for prefix in ["K", "ſ"]:
        add("case-folding", prefix)
    for character in formats:
        add("Cf", character)
        add("ASCII-then-Cf", "a" + character)
    for prefix in ["ｏ", "a\u200b", "K", "ſ", "а", "ο", "a\u0301"]:
        for number in [12, 34]:
            for token in [f"gh-{number}", f"GH-{number}"]:
                samples.append(("own-number", prefix + token, True))
    for text in ["Kgh-5", "sgh-5", "a@octocat", "agh-5", "aGH-5"]:
        samples.append(("ASCII-control", text, True))
    # Verify code masking and normalization-only detection, alongside raw boundaries.
    for text in ["`ｏ@octocat Kgh-5`", "`a\u200b@octocat a\u200bgh-5`"]:
        samples.append(("code-control", text, True))
    for text in ["＠octocat", "ＧＨ-5", "g\u200bh-5"]:
        samples.append(("normalized-only", text, False))

    checked = subprocess.run(
        ["cargo", "run", "--quiet", "--locked", "-p", "tl3-github", "--example", "check_prose"],
        cwd=root, input="".join(json.dumps(text) + "\n" for _, text, _ in samples),
        text=True, capture_output=True, check=True,
    )
    results = [json.loads(line) for line in checked.stdout.splitlines()]
    assert len(results) == len(samples)
    totals = collections.defaultdict(collections.Counter)
    failures = []
    for offset in range(0, len(samples), 50):
        batch = samples[offset:offset + 50]
        text = "\n\n".join(
            f"TL3PROBE{offset + i:06d} {sample}" for i, (_, sample, _) in enumerate(batch)
        )
        rendered = subprocess.run(
            ["gh", "api", "-X", "POST", "markdown", "--input", "-"],
            input=json.dumps({"mode": "gfm", "context": "RobTand/tl3-sandbox", "text": text}),
            text=True, capture_output=True, check=True,
        )
        parser = Paragraphs()
        parser.feed(rendered.stdout)
        assert len(parser.rows) == len(batch), "GitHub changed paragraph boundaries"
        for i, ((group, sample, allowed), row) in enumerate(zip(batch, parser.rows, strict=True)):
            assert row["text"].startswith(f"TL3PROBE{offset + i:06d} ")
            result = results[offset + i]
            counts = totals[group]
            counts["samples"] += 1
            counts["linked"] += bool(row["links"])
            counts["accepted"] += result["accepted"]
            bypass = bool(row["links"]) and result["accepted"] and group != "own-number"
            unexpected_rejection = allowed and not result["accepted"]
            missed_normalization = group == "normalized-only" and result["accepted"]
            counts["bypasses"] += bypass
            counts["unexpected_rejections"] += unexpected_rejection
            if bypass or unexpected_rejection or missed_normalization:
                failures.append({"group": group, "sample": sample, "rendered": row, "checker": result})
        print(json.dumps({"completed": offset + len(batch)}), flush=True)
    print(json.dumps({
        "unicode_version": "17.0.0", "unicode_data_sha256": hashlib.sha256(data).hexdigest(),
        "Cf_characters": len(formats), "total_samples": len(samples),
        "groups": totals, "failures": failures,
    }, ensure_ascii=True, sort_keys=True), flush=True)
    assert not failures, f"{len(failures)} renderer/checker disagreements"


if __name__ == "__main__":
    main()
