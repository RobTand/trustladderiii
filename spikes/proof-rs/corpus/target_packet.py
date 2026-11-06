"""Collect GB10 class facts through the published PB package in an admitted action."""
import json
from pathlib import Path
import sys

runtime = Path('/mnt/shared/prismabuild-fleet/repo').resolve()
sys.path.insert(0, str(runtime / 'src'))
from prismabuild.target_evidence import collect_packet

print(json.dumps(collect_packet(), sort_keys=True))
