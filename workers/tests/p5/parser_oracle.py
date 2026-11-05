"""Batch oracle for proptest-generated raw headers, using the frozen Python."""
import json
import sys
from types import SimpleNamespace
from quota_reference import reset_ms

rows = json.load(sys.stdin)
results = []
for row in rows:
    error = SimpleNamespace(response=SimpleNamespace(status_code=429, headers=row["headers"]))
    results.append(reset_ms(error, float(row["now"])))
print(json.dumps(results))
