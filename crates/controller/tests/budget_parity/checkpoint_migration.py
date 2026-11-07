"""Exact PB inflight checkpoint migration for the frozen budget I/O oracle."""
from copy import deepcopy
from difflib import SequenceMatcher
import json


ADDED = [', ', '"backend"', ': ', '"prismabuild"']
CHECKPOINT_FIELDS = {'version', 'generation', 'start', 'messages', 'budget_exhausted',
                     'allowance', 'inflight', 'worktree'}


class CheckpointMigration:
    def __init__(self, reference):
        self.reference = reference[0]
        additions, documents = {}, []
        writes = None
        for index, call in enumerate(self.reference):
            if call[0] == 'open':
                writes = []
            elif call[0] == 'write_chunk':
                assert writes is not None
                writes.append((index, call))
            elif call[0] == 'flush':
                assert writes
                chunks = [item[1][2] for item in writes]
                value = json.loads(''.join(chunks))
                encoder = json.JSONEncoder(ensure_ascii=False, allow_nan=False)
                assert list(encoder.iterencode(value)) == chunks, 'reference JSON chunks changed'
                migrated = deepcopy(value)
                if isinstance(value, dict) and 'inflight' in value:
                    assert set(value) == CHECKPOINT_FIELDS
                    for entries in migrated['inflight'].values():
                        assert isinstance(entries, list)
                        for entry in entries:
                            assert set(entry) == {'base', 'key', 'tree', 'identity'}
                            entry['backend'] = 'prismabuild'
                desired = list(encoder.iterencode(migrated))
                spans = []
                for kind, start, end, new_start, new_end in SequenceMatcher(
                        a=chunks, b=desired, autojunk=False).get_opcodes():
                    if kind == 'equal':
                        continue
                    assert kind == 'insert' and start == end and desired[new_start:new_end] == ADDED
                    position, original = writes[start]
                    assert position not in additions
                    additions[position] = [['write_chunk', original[1], chunk] for chunk in ADDED]
                    offset = sum(map(len, desired[:new_start]))
                    spans.append((offset, offset + sum(map(len, ADDED))))
                documents.append((''.join(chunks), ''.join(desired), spans))
                writes = None
        self.documents = documents
        self.boundaries, self.calls, self.extra_prefixes = [0], [], []
        old_chunks = new_chunks = 0
        for index, call in enumerate(self.reference):
            for added in additions.get(index, []):
                self.calls.append(added)
                new_chunks += 1
                self.extra_prefixes.append((old_chunks, new_chunks))
            self.calls.append(call)
            old_chunks += call[0] == 'write_chunk'
            new_chunks += call[0] == 'write_chunk'
            self.boundaries.append(len(self.calls))
        self.additions = additions

    def project_buffer(self, value):
        possibilities = set()
        for original, migrated, spans in self.documents:
            if migrated.startswith(value):
                cursor, pieces = 0, []
                for start, end in spans:
                    pieces.append(value[cursor:start])
                    cursor = end
                    if end >= len(value):
                        break
                pieces.append(value[cursor:])
                projected = ''.join(pieces)
                assert original.startswith(projected)
                possibilities.add(projected)
        assert len(possibilities) == 1, 'unknown or ambiguous checkpoint prefix'
        return possibilities.pop()

    def compare(self, reference, current, *, inserted_prefix=False):
        count = len(reference[0])
        assert reference[0] == self.reference[:count], 'reference execution changed before crash'
        boundary = self.boundaries[count]
        ends = [boundary]
        if inserted_prefix:
            ends.extend(range(boundary + 1, boundary + len(self.additions.get(count, [])) + 1))
        assert any(current[0] == self.calls[:end] for end in ends), 'raw call or authorized metadata mismatch'
        assert reference[3] == current[3], 'durability points changed'
        assert reference[2] == current[2], 'outcome changed'
        assert reference[1].keys() == current[1].keys()
        for group, files in reference[1].items():
            assert files.keys() == current[1][group].keys(), 'checkpoint filenames changed'
            for path, contents in files.items():
                assert contents == self.project_buffer(current[1][group][path]), (group, path)


def refusal_controls():
    value = dict(version=1, generation=1, start=0, messages=[], budget_exhausted=False,
                 allowance=None, inflight={'call': [dict(base=False, key='action', tree='tree', identity={})]}, worktree={})
    chunks = list(json.JSONEncoder(ensure_ascii=False, allow_nan=False).iterencode(value))
    calls = [['open', '/visit/maker.tmp', 'w'], *[['write_chunk', '/visit/maker.tmp', part] for part in chunks],
             ['write', '/visit/maker.tmp'], ['flush', '/visit/maker.tmp'], ['fsync', '/visit/maker.tmp']]
    reference = (calls, {'buffered': {'/visit/maker.tmp': ''.join(chunks)}}, 'done', [('flush', '/visit/maker.tmp')])
    migration = CheckpointMigration(reference)
    current = (migration.calls, {'buffered': {'/visit/maker.tmp': migration.documents[0][1]}}, 'done', reference[3])
    migration.compare(reference, current)
    for fault in ('value', 'location', 'extra', 'missing', 'bytes', 'reorder', 'durability', 'outcome'):
        changed = deepcopy(list(current))
        position = next(i for i, call in enumerate(changed[0]) if call[0] == 'write_chunk' and call[2] == '"backend"')
        if fault == 'value':
            changed[0][position + 2][2] = '"flux"'
        elif fault == 'location':
            field = changed[0][position - 1:position + 3]
            del changed[0][position - 1:position + 3]
            root_close = max(i for i, call in enumerate(changed[0]) if call[0] == 'write_chunk')
            changed[0][root_close:root_close] = field
        elif fault == 'extra':
            changed[0].insert(position, ['write_chunk', '/visit/maker.tmp', '"extra"'])
        elif fault == 'missing':
            del changed[0][position]
        elif fault == 'bytes':
            changed[1]['buffered']['/visit/maker.tmp'] = changed[1]['buffered']['/visit/maker.tmp'].replace('"tree"', '"other"')
        elif fault == 'reorder':
            changed[0][-1], changed[0][-2] = changed[0][-2], changed[0][-1]
        elif fault == 'durability':
            changed[3] = []
        else:
            changed[2] = 'changed'
        try:
            migration.compare(reference, changed)
        except AssertionError:
            continue
        raise AssertionError('checkpoint migration accepted ' + fault)
