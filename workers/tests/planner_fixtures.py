"""Shared planner requests, fake models and protocol assertions."""

import copy
import json
import os
import shutil
from pathlib import Path

import pytest
from langchain_core.language_models.fake_chat_models import FakeMessagesListChatModel
from langchain_core.messages import AIMessage

import worker_process
from worker_process import start_worker


LEAF = {
    "kind": "leaf", "estimated_exploration_turns": 0,
    "files": ["greeting.py", "tests/test_greeting.py"],
    "checks": [
        {"kind": "demonstrating", "node_ids": ["tests/test_greeting.py::test_hello"],
         "goal_ids": [0]},
        {"kind": "behavior-preserving", "node_ids": ["tests/test_existing.py::test_api"],
         "goal_ids": [1]},
        {"kind": "regression", "node_ids": ["tests/test_existing.py::test_existing"],
         "goal_ids": [0, 1]},
    ],
    "no_design_questions": True,
    "lands_on_its_own": True,
}


SPLIT = {
    "kind": "split", "reason": "does_not_fit", "estimated_exploration_turns": 0,
    "pieces": [
        {"brief": "Fix the greeting.", "goal_ids": [0], "files": ["greeting.py"], "estimated_exploration_turns": 0},
        {"brief": "Keep the API.", "goal_ids": [1], "files": ["greeting.py", "new/api.py"], "estimated_exploration_turns": 0},
    ],
}


CANT_MEET = {"kind": "cant_meet", "report": "The required API decision is unresolved.",
             "files": []}


class PlannerModel(FakeMessagesListChatModel):
    calls: list = []
    offered: bool = False

    def bind_tools(self, tools, **kwargs):
        self.offered = bool(tools)
        return self

    def _generate(self, messages, **kwargs):
        self.calls.append((copy.deepcopy(messages), self.offered))
        self.offered = False
        return super()._generate(messages, **kwargs)


def planner_model(monkeypatch, *replies):
    from tl3_workers import runtime

    model = PlannerModel(responses=[AIMessage(content=json.dumps(reply)) for reply in replies], calls=[])
    monkeypatch.setattr(runtime, 'create_model', lambda _: model)
    return model


@pytest.fixture(scope="session", autouse=True)
def cache_tokenizer(tmp_path_factory):
    # Bazel supplies the pinned tokenizer as a declared input. Copy it into the
    # isolated session cache before socket denial, then verify it as usual.
    directory = tmp_path_factory.mktemp("tokenizer")
    if cache := os.environ.get("TIKTOKEN_CACHE_DIR"):
        source = Path(cache) / "fb374d419588a4632f3f557e76b4b70aebbca790"
        if source.is_file():
            shutil.copyfile(source, directory / source.name)
    with pytest.MonkeyPatch.context() as patch:
        patch.setenv("TIKTOKEN_CACHE_DIR", str(directory))
        from tiktoken.load import read_file_cached
        from tl3_workers.forecast import TOKENIZER_URL, TOKENIZER_SHA256
        read_file_cached(TOKENIZER_URL, TOKENIZER_SHA256)
        yield


@pytest.fixture(autouse=True)
def tool_capable_fake(monkeypatch):
    # Check the shared fake binds precisely the planner's read-only tools.
    setup = '''
from langchain_core.language_models.fake_chat_models import FakeMessagesListChatModel
def bind_tools(self, tools, **kwargs):
    assert {tool.name for tool in tools} == {'list', 'read', 'glob', 'grep', 'rehearse_forcing'}
    assert all(tool.description for tool in tools)
    return self
FakeMessagesListChatModel.bind_tools = bind_tools
'''
    monkeypatch.setattr(worker_process, "LAUNCH", setup + worker_process.LAUNCH)


@pytest.fixture
def planner_request(tmp_path):
    worktree = tmp_path / "worktree"
    worktree.mkdir()
    (worktree / "greeting.py").write_text("hello world\n")
    (worktree / "tests").mkdir()
    (worktree / "tests/test_existing.py").write_text(
        "def test_api(): assert True\ndef test_existing(): assert True\n")
    return {
        "protocol": 1,
        "operation": {"node": 1, "generation": 0, "kind": "planning"},
        "role": "planner",
        "model": {"base_url": "http://model.invalid/v1", "name": "fixture-model",
                  "effort": "high", "budget_tokens": 32000},
        "key_file": str(tmp_path / "never-read-key"),
        "inputs": {
            "must_split": False,
            "findings": [],
            "protected_paths": [".github/**", ".mergify.yml", "**/conftest.py",
                                "pytest.ini", "tox.ini", "noxfile.py", "setup.cfg",
                                "pyproject.toml"],
            "ingest": {
                "kind": "brief", "brief": "hello world",
                "goals": [{"description": "Return hello.", "verification": "test"},
                          {"description": "Keep the API clear.", "verification": "review"}],
                "memory_ids": [],
            },
            "forecast": {"fit_tokens": 100000, "context_tokens": 150000,
                         "overhead_tokens": {"prompt": 10, "making": 20, "review": 30}},
        },
        "worktree": str(worktree),
    }


def run_reply(tmp_path, request, reply, calls=()):
    turns = []
    if calls:
        turns.append({"content": "", "tool_calls": list(calls)})
    turns.append({"content": json.dumps(reply)})
    return start_worker(tmp_path, request, turns)


def assert_rejected(result, phase="run", message=None):
    assert result.exit_code == 1
    assert not any(event["event"] == "reply" for event in result.events)
    expected = f"tl3_workers: {phase} failed"
    if message:
        expected = f"tl3_workers: planner: {message}\n{expected}"
    assert result.stderr.strip() == expected


def assert_rule_report(result, reason=""):
    assert result.exit_code == 0, result.stderr
    violations = result.events[-1]["reply"]["violations"]
    assert violations and any(reason in report for report in violations)


PID_FORCING = {
    "symptom": "A real, unrelated process holds PID 4242, so the sampler cannot decide whether it left.",
    "environment": {},
    "patches": [{"target": "sampler.process_in_scope", "raises": "ValueError"}],
}


@pytest.fixture
def deflake_worktree(planner_request, monkeypatch):
    from tl3_workers.forecast import Forecaster

    root = Path(planner_request['worktree'])
    (root / 'tests').mkdir(exist_ok=True)
    (root / 'tests/test_sampler.py').write_text(
        'import sampler\n\n\ndef test_departed_process_is_not_live():\n'
        '    assert sampler.live_after([4242], [], "/fake") == 0\n')
    monkeypatch.chdir(root)
    monkeypatch.setattr(Forecaster, 'measure', lambda *a: {'fits': True})
    reply = copy.deepcopy(LEAF)
    reply['files'] = ['tests/test_sampler.py']
    reply['checks'] = [{'kind': 'demonstrating', 'goal_ids': [0, 1], 'forcing': copy.deepcopy(PID_FORCING),
                        'node_ids': ['tests/test_sampler.py::test_departed_process_is_not_live']}]
    return reply


MEMORIES = [{"id": "lesson-1", "kind": "lesson", "repo": "RobTand/tl3-sandbox",
             "content": "greet() returns 'hi'."},
            {"id": "run-1", "kind": "run", "content": "An earlier run read greeting.py."}]


FLAG = {"id": "lesson-1", "claim": "greet() returns 'hi'", "reason": "greeting.py says hello world.",
        "evidence": {"path": "greeting.py", "start_line": 1, "end_line": 1, "quote": "hello world"}}


DIGESTS = 'tests/test_digests.py'


FROZEN = f'{DIGESTS}::test_digests_imports_only_the_standard_library'


SIBLING = f'{DIGESTS}::test_digest_is_stable'


@pytest.fixture
def digests_plan(planner_request, monkeypatch):
    # #438: PQ #2662's shape. A goal extends an existing test that a check freezes.
    from tl3_workers.forecast import Forecaster

    root = Path(planner_request['worktree'])
    (root / 'digests.py').write_text('import hashlib\n')
    (root / DIGESTS).write_text(
        'import pytest\n\n'
        'def test_digests_imports_only_the_standard_library():\n    assert True\n\n'
        '@pytest.mark.parametrize("n", [1, 2])\n'
        'def test_digest_is_stable(n):\n    assert n\n')
    planner_request['inputs']['ingest']['goals'] = [
        {'description': 'Extend `test_digests_imports_only_the_standard_library` so the '
                        'standalone-loaded module force-executes its lazy imports.',
         'verification': 'test'},
        {'description': 'Keep digests stable.', 'verification': 'test'}]
    (root / 'pyproject.toml').write_text(
        '[tool.tl3.harness]\ncommand = ["python", "-m", "pytest"]\ntest-files = ["tests/**/*.py"]\n')
    monkeypatch.chdir(root)
    monkeypatch.setattr(Forecaster, 'measure', lambda *a: {'fits': True})
    return {
        'kind': 'leaf', 'estimated_exploration_turns': 0, 'files': ['digests.py', DIGESTS],
        'checks': [
            {'kind': 'demonstrating', 'node_ids': [f'{DIGESTS}::test_lazy_imports_run'], 'goal_ids': [0]},
            {'kind': 'regression', 'node_ids': [SIBLING], 'goal_ids': [1]},
            {'kind': 'regression', 'node_ids': [FROZEN], 'goal_ids': [0]},
        ],
        'no_design_questions': True, 'lands_on_its_own': True,
    }


def planned(planner_request, reply):
    from tl3_workers.roles.planner import check_reply

    check_reply(planner_request['inputs'], reply)
    return reply['violations']




FORCING = {'symptom': 'Another process holds the module lock while digests import.', 'environment': {},
           'patches': [{'target': 'digests.LOCK_HELD', 'value': True}]}


NAME = 'test_digests_imports_only_the_standard_library'


@pytest.fixture
def request_149_plan(planner_request):
    root = Path(planner_request['worktree'])
    (root / 'tests/test_measurement_census_reader_fence_1498.py').write_text(
        'def test_a_busy_fence_refuses_before_any_transition_or_host_lock():\n'
        '    elapsed = 0.0\n    assert elapsed < 5.0, elapsed\n')
    planner_request['inputs']['ingest']['goals'] = [
        {'description': 'Deflake the busy-fence test with virtual polling time, preserving refusal evidence, no transition or host lock before refusal, and reuse after release.',
         'verification': 'test'},
        {'description': 'Keep native thread bounds at one.', 'verification': 'test'},
    ]
    return {
        'kind': 'leaf', 'estimated_exploration_turns': 0, 'files': ['tests/test_measurement_census_reader_fence_1498.py'],
        'checks': [{'kind': 'demonstrating', 'goal_ids': [0, 1],
                    'node_ids': ['tests/test_measurement_census_reader_fence_1498.py::test_a_busy_fence_refuses_before_any_transition_or_host_lock'],
                    'forcing': {
                        'symptom': 'A slow locked_census adds four seconds while a busy reader fence is polled; the old elapsed < 5.0 assertion measures scheduler stalls.',
                        'environment': {'OMP_NUM_THREADS': '1', 'MKL_NUM_THREADS': '1',
                                        'OPENBLAS_NUM_THREADS': '1'},
                        'patches': [{'target': 'prismabuild._measurement_reservation.locked_census',
                                     'delay_seconds': 4}],
                    }}],
        'no_design_questions': True, 'lands_on_its_own': True,
    }


PBWAIT_TESTS = 'tests/test_pbwait_finds_a_later_submission.py'


LATE = f'{PBWAIT_TESTS}::test_finds_a_later_submission'


CONTROL = f'{PBWAIT_TESTS}::test_a_key_nothing_ever_records_still_gives_up_as_waiting'


@pytest.fixture
def request156(planner_request, monkeypatch):
    planner_request['inputs']['repair_plan_consistency'] = True
    root = Path(planner_request['worktree'])
    (root / PBWAIT_TESTS).write_text(
        'def test_finds_a_later_submission():\n    assert True\n\n'
        'def test_a_key_nothing_ever_records_still_gives_up_as_waiting():\n'
        '    wait_s = 0.0\n    verdict = "waiting"\n    assert verdict == "waiting"\n')
    planner_request['inputs']['ingest']['goals'] = [
        {'description': 'Find a later submission deterministically.', 'verification': 'test'},
        {'description': 'Keep the no-submission control returning waiting with wait_s=0.0.',
         'verification': 'test'},
    ]
    tool = {'tool': 'run_tests', 'executor': 'prismabuild_pytest',
            'runner_features': ['base_head', 'native_threads_1'],
            'artifacts': ['logs', 'terminal_verdict', 'cas_receipt'],
            'methods': ['red_green'], 'protected_files': []}
    planner_request['inputs']['tool_capabilities'] = [tool]
    planner_request['inputs']['ingest']['evidence_contract'] = [
        {**copy.deepcopy(tool), 'requirement': 'Retain base/head proof and native thread bounds.'}]
    monkeypatch.chdir(root)
    return {
        'kind': 'leaf', 'estimated_exploration_turns': 0, 'files': ['greeting.py', PBWAIT_TESTS],
        'edits_tests': [LATE, CONTROL],
        'checks': [{'kind': 'demonstrating', 'node_ids': [LATE], 'goal_ids': [0, 1]}],
        'no_design_questions': True, 'lands_on_its_own': True, 'evidence_contract': [],
    }


RELAY_TESTS = 'tests/test_docker_shim_relays_termination_1599.py'


SMOKE = f'{RELAY_TESTS}::test_a_term_reaches_the_client_and_the_receipt_names_the_container'


CREATED = f'{RELAY_TESTS}::test_a_created_container_that_cannot_be_removed_is_start_unresolved'


INVENTED = [f'{RELAY_TESTS}::test_a_created_container_whose_removal_is_denied_is_start_unresolved_in_virtual_time',
            f'{RELAY_TESTS}::test_a_mutant_treating_created_as_stopped_fails_the_removal_denied_scenario']


@pytest.fixture
def request162(planner_request, monkeypatch):
    planner_request['inputs']['repair_plan_consistency'] = True
    root = Path(planner_request['worktree'])
    (root / RELAY_TESTS).write_text(
        'def test_a_term_reaches_the_client_and_the_receipt_names_the_container():\n    assert True\n\n'
        'def test_a_created_container_that_cannot_be_removed_is_start_unresolved():\n    assert True\n')
    (root / 'pyproject.toml').write_text('[tool.tl3.harness]\ntest-files = ["tests/**/*.py"]\n')
    planner_request['inputs']['ingest']['goals'] = [
        {'description': 'Drive rejected created-container removal with virtual time.', 'verification': 'test'},
        {'description': 'Reject a mutant treating Running=False as proof of stop.', 'verification': 'test'},
        {'description': 'Keep the OS signal smoke check.', 'verification': 'test'},
    ]
    monkeypatch.chdir(root)
    return {
        'kind': 'leaf', 'estimated_exploration_turns': 0, 'files': [RELAY_TESTS], 'edits_tests': [CREATED],
        'checks': [{'kind': 'demonstrating', 'node_ids': [CREATED, *INVENTED], 'goal_ids': [0, 1],
                    'forcing': {'symptom': 'Delay daemon replies during created-container cleanup.',
                                'environment': {},
                                'patches': [{'target': 'shim.DAEMON_DELAY', 'value': 1}]}},
                   {'kind': 'regression', 'node_ids': [SMOKE], 'goal_ids': [2]}],
        'no_design_questions': True, 'lands_on_its_own': True,
    }


EXPORT_TESTS = 'tests/test_paced_exports_share_a_tier.py'


FINISHED = f'{EXPORT_TESTS}::test_a_finished_export_files_its_rate_and_prices_the_next_seal'


PROPOSED_EXPORTS = [f'{EXPORT_TESTS}::test_a_fast_writer_is_held_and_flushed_under_virtual_time',
                    f'{EXPORT_TESTS}::test_a_delayed_writer_files_a_sub_one_rate_and_seals_one',
                    f'{EXPORT_TESTS}::test_a_virtual_receipt_prices_ceiled_and_capped_by_the_offer']


@pytest.fixture
def request165(planner_request, monkeypatch):
    planner_request['inputs']['repair_plan_consistency'] = True
    planner_request['inputs']['tool_capabilities'] = [
        {'tool': 'run_tests', 'executor': 'prismabuild_pytest',
         'runner_features': ['base_head', 'native_threads_1'],
         'artifacts': ['logs', 'terminal_verdict', 'cas_receipt'],
         'methods': ['red_green', 'forced_reproduction.patches'], 'protected_files': []}]
    root = Path(planner_request['worktree'])
    (root / EXPORT_TESTS).write_text(
        'def test_a_finished_export_files_its_rate_and_prices_the_next_seal():\n    assert True\n')
    (root / 'pyproject.toml').write_text('[tool.tl3.harness]\ntest-files = ["tests/**/*.py"]\n')
    planner_request['inputs']['ingest']['goals'] = [
        {'description': description, 'verification': 'review' if index == 6 else 'test',
         **({'stage': 'publishing', 'publication': {'non_draft': True, 'body_lines': [], 'proof': True}}
            if index == 6 else {})}
        for index, description in enumerate([
            'Use virtual time through the real export, pacer and receipt-writing path.',
            'A fast writer must incur a virtual pacing hold and flush.',
            'A delayed writer must file a sub-1 MB/s rate without a required sleep.',
            'Keep real CAS requests, receipt filing and next-export seal pricing.',
            'Assert independent literal rates and prices, not pricing-function tautologies.',
            'Keep ceil, minimum-one and offer-cap pricing and tier token totals within the offer.',
            'Report focused base/head proof and receipt hashes in the PR.',
            'Keep launch and receipt integration smoke coverage.',
            'Preserve the seal ceiling without introducing a scheduler throughput floor.',
            'Deterministic schedules must fail faulty pacing or receipt pricing.',
        ])]
    planner_request['inputs']['ingest']['evidence_errors'] = []
    monkeypatch.chdir(root)
    return {
        'kind': 'leaf', 'estimated_exploration_turns': 0, 'files': [EXPORT_TESTS], 'edits_tests': [FINISHED], 'evidence_contract': [],
        'checks': [{'kind': 'demonstrating', 'node_ids': [FINISHED, *PROPOSED_EXPORTS],
                    'goal_ids': [0, 1, 2, 3, 4, 5, 7, 8, 9],
                    'forcing': {'symptom': 'Deschedule the export writer before its block is counted; '
                                           'the old rate floor and positive-hold assertions fail.',
                                'environment': {},
                                'patches': [{'target': 'prismabuild.produced_spool.ExportPacer.wrote',
                                             'delay_seconds': 2}]}}],
        'no_design_questions': True, 'lands_on_its_own': True,
    }


FORGED_FORECAST = {
    "tokenizer": "o200k_base", "brief_tokens": 1, "file_tokens": {},
    "overhead_tokens": 10, "tokens": 11, "fit_tokens": 100,
    "context_tokens": 150, "fits": True,
}


def execution_split():
    return {"kind": "split", "reason": "execution_budget", "estimated_tool_turns": 65, "estimated_exploration_turns": 0,
            "pieces": [
                {"brief": "Implement and verify the greeting, including metadata.", "goal_ids": [0],
                 "files": ["greeting.py"], "estimated_tool_turns": 25, "estimated_exploration_turns": 0},
                {"brief": "Preserve the API and run final regressions.", "goal_ids": [1],
                 "files": ["greeting.py", "new/api.py"], "estimated_tool_turns": 25, "estimated_exploration_turns": 0}]}
