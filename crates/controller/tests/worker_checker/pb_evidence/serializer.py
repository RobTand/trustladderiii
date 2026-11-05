"""Compare the production renderer with Python for generated Unicode and numeric facts."""
import json
import sys

count = 0
for raw in json.loads(sys.argv[1]):
    value = json.loads(raw)
    for ascii in (True, False):
        for compact in (True, False):
            print(json.dumps({'request': {'operation': 'python-json', 'value': value,
                                         'ascii': ascii, 'compact': compact}}), flush=True)
            response = json.loads(sys.stdin.readline())
            expected = json.dumps(value, sort_keys=True, ensure_ascii=ascii,
                                  separators=(',', ':') if compact else None, allow_nan=False)
            assert response == {'return': expected}, (raw, response, expected)
            count += 1
print(json.dumps({'done': count}), flush=True)
