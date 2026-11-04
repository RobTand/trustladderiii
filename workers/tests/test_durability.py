"""#768: retain real persistence barriers behind the durability marker."""
import json
import os
import stat

import pytest

from tl3_workers import facts, memory, session, tracing


@pytest.mark.parametrize('name', ['fsync', 'fdatasync'])
def test_content_checks_skip_disk_barriers(name):
    assert getattr(os, name)(-1) is None


@pytest.mark.durability
@pytest.mark.parametrize('name', ['fsync', 'fdatasync'])
def test_durability_checks_keep_real_disk_barriers(name):
    with pytest.raises(OSError):
        getattr(os, name)(2**31 - 1)


@pytest.mark.durability
@pytest.mark.parametrize('writer', ['session', 'facts', 'summary'])
def test_atomic_write_syncs_before_publication_and_then_syncs_directory(tmp_path, monkeypatch, writer):
    """#768: checkpoints, completion facts and summaries keep their sync order."""
    with pytest.raises(OSError):
        os.fsync(2**31 - 1)
    path = tmp_path / 'checkpoint.json'
    events = []
    original_sync, original_replace = os.fsync, os.replace

    def sync(fd):
        original_sync(fd)
        events.append('directory' if stat.S_ISDIR(os.fstat(fd).st_mode) else 'file')

    def replace(source, target):
        assert events == ['file']
        assert json.loads(source.read_text()) == expected
        original_replace(source, target)
        events.append('replace')

    monkeypatch.setattr(os, 'fsync', sync)
    monkeypatch.setattr(os, 'replace', replace)
    expected = {'value': 'saved'}
    if writer == 'summary':
        summary = memory.Summary({'memory': {'summary_file': str(path)}})
        expected = summary.value
        summary.save()
    else:
        (session.save if writer == 'session' else facts.publish)(path, expected)
    assert events == ['file', 'replace', 'directory']
    assert json.loads(path.read_text()) == expected
    assert list(tmp_path.iterdir()) == [path]


@pytest.mark.durability
def test_span_log_syncs_before_waking_the_shipper(tmp_path, monkeypatch):
    """#430/#768: the shipper wake follows the completed log durability barrier."""
    with pytest.raises(OSError):
        os.fdatasync(2**31 - 1)
    log = tracing.SpanLog(str(tmp_path / 'spans.jsonl'))
    read, log.wake = os.pipe()
    events = []
    original_sync, original_write = os.fdatasync, os.write

    def sync(fd):
        original_sync(fd)
        assert fd == log.fd
        events.append('sync')

    def write(fd, data):
        if fd == log.wake:
            assert events == ['sync']
            events.append('wake')
        return original_write(fd, data)

    monkeypatch.setattr(os, 'fdatasync', sync)
    monkeypatch.setattr(os, 'write', write)
    try:
        log.append({'kind': 'fixture'})
        assert events == ['sync', 'wake']
        assert os.read(read, 1) == b'\n'
        assert json.loads((tmp_path / 'spans.jsonl').read_text()) == {'kind': 'fixture'}
    finally:
        os.close(read)
        os.close(log.wake)
        os.close(log.fd)
