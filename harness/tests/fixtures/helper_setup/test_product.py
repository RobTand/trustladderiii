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
    if not isinstance(row, dict):
        raise ValueError('missing claimed row')
    outcome = q.execute(row, timeout_s=240)
    assert outcome.returncode == 0, 'mover failed'
    return outcome
