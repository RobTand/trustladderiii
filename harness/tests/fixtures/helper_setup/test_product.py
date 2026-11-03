from types import SimpleNamespace


class Records:
    def __init__(self, row):
        self.row = row

    def item(self, state, mover):
        assert state == 'CLAIMED'
        assert mover == 'mover-key'
        return self.row


class Queue:
    def __init__(self, code):
        self.records = Records({'key': 'mover-key'})
        self.result = SimpleNamespace(returncode=code, stdout='stdout-marker' + 'x' * 200,
                                      stderr='stderr-cause')
        self.cleaned = False

    def execute(self, row, timeout_s):
        assert row == {'key': 'mover-key'}
        assert timeout_s == 240
        self.cleaned = True
        return self.result


def _failed_queue():
    return Queue(7)


def _missing_row():
    q = _failed_queue()
    q.records.row = None
    return q


def _noncallable():
    q = _failed_queue()
    q.execute = None
    return q


def _setup_asserts():
    raise AssertionError('setup has not reached the target')


def _wrong_exception():
    raise TypeError('setup cannot construct a queue')


def _execute_mover(q, mover):
    row = q.records.item('CLAIMED', mover)
    assert isinstance(row, dict), 'missing claimed row'
    outcome = q.execute(row, timeout_s=240)
    assert outcome.returncode == 0, 'mover failed'
    return outcome


class DiskRecords:
    def __init__(self, root):
        self.path = root / 'claimed.json'

    def item(self, state, mover):
        import json
        assert state == 'CLAIMED'
        if not self.path.exists():
            return None
        row = json.loads(self.path.read_text())
        return row if row['key'] == mover else None


class DiskQueue(Queue):
    def __init__(self, root):
        super().__init__(7)
        self.records = DiskRecords(root)
        self.root = root

    def execute(self, row, timeout_s):
        try:
            return super().execute(row, timeout_s)
        finally:
            self.records.path.unlink()
            (self.root / 'cleaned').touch()


def _queue(tmp_path):
    import json
    from pathlib import Path
    root = tmp_path / 'queue'
    root.mkdir()
    with Path('fixture-roots.jsonl').open('a') as stream:
        stream.write(json.dumps(str(tmp_path)) + '\n')
    return DiskQueue(root)


def _staged_world(tmp_path):
    import json
    q = _queue(tmp_path)
    q.records.path.write_text(json.dumps({'key': 'mover-key'}))
    return q


def _two_real_generations(tmp_path_factory):
    return _staged_world(tmp_path_factory.mktemp('generations'))


def _both_fixtures(tmp_path, *, tmp_path_factory):
    tmp_path_factory.mktemp('extra')
    return _staged_world(tmp_path)


def _fixture_setup_asserts(tmp_path):
    _staged_world(tmp_path)
    raise AssertionError('setup failed after creating the claimed row')


def _fixture_method_raises(tmp_path):
    q = _staged_world(tmp_path)
    def execute(*args, **kwargs):
        raise AssertionError('execute did not return')
    q.execute = execute
    return q


def _too_many_paths(tmp_path_factory):
    import json
    from pathlib import Path
    with Path('fixture-roots.jsonl').open('a') as stream:
        stream.write(json.dumps(str(tmp_path_factory.getbasetemp())) + '\n')
    for _ in range(17):
        tmp_path_factory.mktemp('extra')
    return Queue(7)


def _positional_fixture(tmp_path, /):
    return _staged_world(tmp_path)


def _defaulted_factory(code=7):
    return Queue(code)
