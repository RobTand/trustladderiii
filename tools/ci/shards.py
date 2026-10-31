"""Deterministic weighted partitioning for hosted CI jobs."""
import json
from pathlib import Path


def weights(language):
    return json.loads(Path(__file__).with_name('hosted-shards.json').read_text())[language]


def partition(names, total, weights):
    if total < 1:
        raise ValueError('Shard count must be positive')
    if len(names) != len(set(names)):
        raise ValueError('Cannot partition duplicate suite names')
    groups, loads = [[] for _ in range(total)], [0.0] * total
    for name in sorted(names, key=lambda name: (-weights.get(name, 5), name)):
        index = min(range(total), key=lambda index: (loads[index], index))
        groups[index].append(name)
        loads[index] += weights.get(name, 5)
    return groups


def parse_shard(value):
    try:
        index, total = map(int, value.split('/'))
    except (ValueError, AttributeError) as error:
        raise ValueError('Shard must be a zero-based INDEX/TOTAL') from error
    if not 0 <= index < total:
        raise ValueError('Shard must satisfy 0 <= INDEX < TOTAL')
    return index, total
