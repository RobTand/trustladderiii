#!/usr/bin/env python3
"""Explicit live proxy smoke, not a TL3 test. Print only catalog IDs and usage."""
import json
from pathlib import Path
import sys
import tomllib
import urllib.error
import urllib.request


def main():
    config = tomllib.loads(Path(sys.argv[1]).read_text())["models"]
    # Runtime-only credential loading. Never emit headers, bodies, or exceptions.
    headers = {"Authorization": "Bearer " + Path(config["key_file"]).read_text().strip(),
               "Content-Type": "application/json"}
    request = urllib.request.Request(config["base_url"].rstrip("/") + "/models", headers=headers)
    with urllib.request.urlopen(request, timeout=60) as response:
        catalog = {row["id"] for row in json.load(response)["data"]}
        print(json.dumps({"catalog_http": response.status, "models": sorted(catalog)}), flush=True)
    models = {}
    for ladder in config.values():
        if isinstance(ladder, list):
            for model in ladder:
                models[model["name"]] = model
    failed = False
    for name, model in sorted(models.items()):
        # Select attempt 2's maximum effort when the same model appears twice.
        if name == config["maker"][1]["name"]:
            model = config["maker"][1]
        body = {"model": name, "messages": [{"role": "user", "content": "Reply OK."}],
                "max_completion_tokens": 16}
        if model.get("effort"):
            body["reasoning_effort"] = model["effort"]
        request = urllib.request.Request(config["base_url"].rstrip("/") + "/chat/completions",
                                         data=json.dumps(body).encode(), headers=headers)
        try:
            with urllib.request.urlopen(request, timeout=180) as response:
                data = json.load(response)
                usage = data.get("usage", {})
                row = {"model": name, "http": response.status, "effort": model.get("effort"),
                       "prompt_tokens": usage.get("prompt_tokens"),
                       "completion_tokens": usage.get("completion_tokens"),
                       "total_tokens": usage.get("total_tokens")}
                failed |= not isinstance(row["total_tokens"], int) or row["total_tokens"] <= 0
        except urllib.error.HTTPError as error:
            row = {"model": name, "http": error.code, "total_tokens": None}
            failed = True
        except Exception:
            row = {"model": name, "http": None, "total_tokens": None}
            failed = True
        print(json.dumps(row), flush=True)
    return int(failed)


if __name__ == "__main__":
    try:
        sys.exit(main())
    except Exception:
        sys.exit("Proxy smoke failed; details suppressed")
