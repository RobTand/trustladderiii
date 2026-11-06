"""Concrete #899 source partitions and repairs, shared by row and verdict tests."""
from copy import deepcopy


def partitions():
    safe = {'before': {'app.py': 'def value(): return 1\n', 'tests/test_observer.py': 'def test_control(): pass\n'},
            'after': {'app.py': 'def value(): return 2\n', 'tests/test_observer.py': 'def test_control(): pass\ndef test_observer(): assert True\n'},
            'tests': ['tests/test_observer.py::test_observer'],
            'proof': {'subjects': ['app.py::value'], 'observer_support': [], 'red_message': 'expected'},
            'config': {'test-files': ['tests/*.py']}}
    cases = {}
    def add(rule, bad, good=None):
        cases['proof.' + rule] = {'bad': bad, 'good': good or deepcopy(safe)}
    def change():
        return deepcopy(safe)
    def support(source, expression='helper()'):
        case = change()
        case['after']['tests/test_observer.py'] = ('def test_control(): pass\n' + source +
            '\ndef test_observer(): assert ' + expression + '\n')
        case['proof']['observer_support'] = ['tests/test_observer.py::' + ('TOKEN' if expression == 'TOKEN' else 'helper')]
        return case
    literal = support('TOKEN = 1', 'TOKEN')
    function = support('def helper(): return 1')
    for rule, source, witness in [
        ('definition_write', 'def helper(): return 1\nhelper.value = 1', function),
        ('definition_use', 'def helper(): return 1\nALIAS = helper', function),
        ('definition_binding', 'def helper(): return 1\nfrom other import helper', function),
        ('literal_initializer', 'TOKEN = lambda: 1', literal),
        ('literal_assignment', 'TOKEN = OTHER = 1', literal),
        ('literal_binding', 'TOKEN = {}', literal),
        ('wildcard_import', 'from other import *\ndef helper(): return 1', function),
        ('namespace_api', 'def helper(): return globals()', function),
        ('decorator', 'import pytest\n@pytest.fixture\ndef helper(): return 1', function),
        ('class_bases', 'class helper(dict): pass', support('class helper: pass')),
        ('context_manager', 'def helper():\n    with open("x"): pass\n    return 1', function),
        ('call_allowlist', 'def helper(value=1): return sorted(value)', function),
        ('percent_format', 'def helper(value=1): return "%s" % value', function),
        ('reflective_attribute', 'def helper(): return "{}".format', function),
        ('member_write', 'def helper(box=None): box["x"] = 1', function),
        ('graft_binding', 'def helper():\n    global EXISTING\n    EXISTING = 1\n    return 1', function),
    ]:
        bad = support(source, 'TOKEN' if rule.startswith('literal_') else 'helper()')
        if rule == 'wildcard_import':
            bad = change()
            bad['after']['support.py'] = 'from other import *\ndef helper(): return 1\n'
            bad['proof']['observer_support'] = ['support.py']
        if rule == 'graft_binding':
            bad['before']['tests/test_observer.py'] += 'EXISTING = 0\n'
        add('support.' + rule, bad, witness)
    bad = change(); bad['destination_exists'] = True
    add('tree.destination_fresh', bad)
    bad = change(); bad['proof']['subjects'] = bad['tests'][:]
    add('subject.overlap', bad)
    bad = change(); bad['after']['app.py'] = bad['before']['app.py']
    add('subject.changed', bad)
    bad = change(); bad['before']['tests/test_observer.py'] = 'A = B = 1\n'; bad['after']['tests/test_observer.py'] = 'A = B = 2\n'
    bad['proof']['subjects'] = ['tests/test_observer.py::A']; bad['tests'] = ['tests/test_observer.py::B']
    add('subject.preserved', bad)
    bad = deepcopy(function); bad['before']['tests/test_observer.py'] += 'def helper(): return 1\n'
    add('support.new', bad, function)
    bad = change(); bad['tests'] = ['tests/test_observer.py']
    add('support.existing_module', bad)
    bad = change(); bad['tests'] = ['tests/test_observer.py::test_missing']
    add('source.definition', bad)
    bad = support('def helper():\n    nonlocal missing\n    return 1')
    add('source.unparsable', bad, function)
    bad = change(); bad['tests'] = ['tests/test_observer.py', *bad['tests']]
    add('observer.overlap', bad)
    bad = change(); bad['config']['test-files'] = ['specs/*.py']
    add('observer.test_path', bad)
    bad = change(); del bad['after']['tests/test_observer.py']
    add('observer.module_missing', bad)
    bad = change(); del bad['before']['tests/test_observer.py']; bad['after']['tests/test_observer.py'] += 'VALUE = 2\n'
    bad['proof']['subjects'] = ['tests/test_observer.py::VALUE']
    add('observer.subject_module', bad)
    bad = change(); bad['after']['tests/test_observer.py'] = 'class TestNew:\n    def subject(self): return 2\n    def test_observer(self): assert True\n'
    bad['tests'] = ['tests/test_observer.py::TestNew::test_observer']; bad['proof']['subjects'] = ['tests/test_observer.py::TestNew::subject']
    add('observer.subject_class', bad)
    bad = change(); bad['before']['tests/test_observer.py'] = 'class TestOld:\n    def test_observer(self): assert False\n'
    bad['after']['tests/test_observer.py'] = bad['before']['tests/test_observer.py'].replace('False', 'True')
    bad['tests'] = ['tests/test_observer.py::TestOld']
    good = deepcopy(bad); good['tests'] = ['tests/test_observer.py::TestOld::test_observer']
    add('observer.exact_method', bad, good)
    bad = change(); bad['before']['tests/test_observer.py'] = 'from other import test_observer\n'
    add('observer.binding_shadow', bad)
    bad = change(); bad['after']['tests/test_observer.py'] += 'def test_missing(): assert MISSING\n'; bad['tests'] = ['tests/test_observer.py::test_missing']
    add('observer.unresolved_symbol', bad)
    bad = change(); bad['before']['tests/test_observer.py'] = 'from other import *\ndef test_observer(): assert True\n'
    bad['after']['tests/test_observer.py'] = 'from other import *\nTOKEN = 1\ndef test_observer(): assert TOKEN\n'
    good = deepcopy(bad); good['after']['tests/test_observer.py'] = 'from other import *\ndef test_observer():\n    TOKEN = 1\n    assert TOKEN\n'
    add('support.literal_wildcard', bad, good)
    bad = change(); del bad['before']['tests/test_observer.py']; bad['after']['pathlib.py'] = ''
    good = deepcopy(bad); del good['after']['pathlib.py']
    add('loader.stdlib_shadow', bad, good)
    return cases
