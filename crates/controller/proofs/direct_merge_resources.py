"""Measure the real #830 proof process and preserve its exit verdict."""
import json
import resource
import subprocess
import sys
import time

started = time.monotonic()
result = subprocess.run(sys.argv[1:], check=False)
print("DIRECT_MERGE_RESOURCES " + json.dumps({
    "seconds": time.monotonic() - started,
    "max_rss_kib": resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss,
    "returncode": result.returncode,
}), flush=True)
raise SystemExit(result.returncode)
