"""Trusted dependency preparation. Submit this script through PrismaBuild."""
import argparse
import json
import os
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("base", help="locally available Ubuntu 26.04 image")
    parser.add_argument("--profile", choices=("sandbox", "prismabuild"), default="sandbox")
    parser.add_argument("--tag")
    args = parser.parse_args()
    args.tag = args.tag or ("tl3-harness:prismabuild" if args.profile == "prismabuild" else "tl3-harness:t5")
    owner = os.environ.get("PRISMABUILD_CONTAINER_OWNER")
    if not owner or not os.environ.get("PRISMABUILD_CONTAINER_MARKER"):
        raise RuntimeError("image preparation requires a PrismaBuild action")
    name = "tl3-image-" + owner[:20]
    system = "gcc libc6-dev bubblewrap" if args.profile == "prismabuild" else ""
    packages = "pytest-xdist==3.8.0 PyYAML==6.0.3 setuptools==80.9.0" if args.profile == "prismabuild" else ""
    install = f"""
set -eu
export DEBIAN_FRONTEND=noninteractive
apt-get update
apt-get install --no-install-recommends -y python3 python3-venv git ca-certificates {system}
python3 -m venv /opt/tl3
/opt/tl3/bin/python -m pip install --no-cache-dir pytest==9.0.2 {packages}
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
