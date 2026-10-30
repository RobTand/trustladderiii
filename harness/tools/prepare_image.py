"""Trusted dependency preparation. Submit this script through PrismaBuild."""
import json
import os
import subprocess
import sys


def main():
    owner = os.environ.get("PRISMABUILD_CONTAINER_OWNER")
    if not owner or not os.environ.get("PRISMABUILD_CONTAINER_MARKER"):
        raise RuntimeError("image preparation requires a PrismaBuild action")
    name = "tl3-image-" + owner[:20]
    install = """
set -eu
export DEBIAN_FRONTEND=noninteractive
apt-get update
apt-get install --no-install-recommends -y python3 python3-venv git ca-certificates
python3 -m venv /opt/tl3
/opt/tl3/bin/python -m pip install --no-cache-dir pytest==9.0.2
/opt/tl3/bin/python -c 'import sys; assert sys.version_info >= (3, 14)'
apt-get clean
rm -rf /var/lib/apt/lists/*
"""
    try:
        subprocess.run(["docker", "run", "--pull=never", "--name", name,
                        "--env", "OMP_NUM_THREADS=1", "--env", "MKL_NUM_THREADS=1",
                        "--env", "OPENBLAS_NUM_THREADS=1", "--entrypoint=/bin/sh",
                        sys.argv[1], "-ec", install], check=True)
        image = subprocess.check_output([
            "docker", "commit", "--change", "ENV PATH=/opt/tl3/bin:/usr/local/bin:/usr/bin:/bin",
            name, "tl3-harness:t5"], text=True).strip()
        print(json.dumps({"container_image": image}), flush=True)
    finally:
        subprocess.run(["docker", "rm", "--force", name], check=False)


if __name__ == "__main__":
    main()
