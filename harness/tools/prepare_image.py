"""Trusted dependency preparation. Run this script in an admitted Flux job."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("base", help="locally available Ubuntu 26.04 image")
    parser.add_argument("--profile", choices=("sandbox", "prismaquant", "tessera"), default="sandbox")
    parser.add_argument("--tag")
    args = parser.parse_args()
    if args.profile == "prismaquant":
        raise ValueError("use flux_prismaquant_image.py with a pinned Python 3.12 base and hashed lock")
    args.tag = args.tag or ("tl3-harness:t5" if args.profile == "sandbox" else f"tl3-harness:{args.profile}")
    owner = os.environ.get("FLUX_JOB_ID")
    if not owner:
        raise RuntimeError("image preparation requires an admitted Flux job")
    name = "tl3-image-" + hashlib.sha256(owner.encode()).hexdigest()[:20]
    system = ""
    pip_install = "/opt/tl3/bin/python -m pip install --no-cache-dir pytest==9.0.2"
    if args.profile in ("prismaquant", "tessera"):
        system = "python3.14-dev gcc g++ make libgomp1 bubblewrap"
        requirements = (Path(__file__).with_name("images") / f"{args.profile}.txt").read_text()
        torch, = (line for line in requirements.splitlines() if line.startswith("torch=="))
        requirements = "\n".join(line for line in requirements.splitlines() if line != torch) + "\n"
        pip_install = f"""cat > /tmp/tl3-requirements.txt <<'TL3_REQUIREMENTS'
{requirements}TL3_REQUIREMENTS
/opt/tl3/bin/python -m pip install --no-cache-dir --index-url https://pypi.org/simple setuptools==80.10.2 wheel==0.48.0
/opt/tl3/bin/python -m pip install --no-cache-dir --index-url https://download.pytorch.org/whl/cpu --no-deps {torch}
/opt/tl3/bin/python -m pip install --no-cache-dir --index-url https://pypi.org/simple --no-build-isolation --no-deps -r /tmp/tl3-requirements.txt
rm /tmp/tl3-requirements.txt"""
    install = f"""
set -eu
export DEBIAN_FRONTEND=noninteractive
apt-get update
apt-get install --no-install-recommends -y python3 python3-venv git ca-certificates {system}
python3 -m venv /opt/tl3
{pip_install}
/opt/tl3/bin/python -c 'import sys; assert sys.version_info >= (3, 14)'
apt-get clean
rm -rf /var/lib/apt/lists/*
"""
    try:
        subprocess.run(["docker", "run", "--pull=never", "--name", name,
                        "--env", "OMP_NUM_THREADS=1", "--env", "MKL_NUM_THREADS=1",
                        "--env", "OPENBLAS_NUM_THREADS=1", "--entrypoint=/bin/sh",
                        args.base, "-ec", install], check=True)
        image = subprocess.check_output([
            "docker", "commit", "--change", "ENV PATH=/opt/tl3/bin:/usr/local/bin:/usr/bin:/bin",
            name, args.tag], text=True).strip()
        print(json.dumps({"container_image": image, "tag": args.tag, "profile": args.profile}), flush=True)
    finally:
        subprocess.run(["docker", "rm", "--force", name], check=False)


if __name__ == "__main__":
    main()
