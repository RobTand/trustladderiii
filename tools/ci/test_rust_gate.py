import sys
import tempfile
import threading
from pathlib import Path
import unittest
from unittest.mock import patch

import rust_gate


class GateTests(unittest.TestCase):
    def test_budget_rejects_slow_tests_without_an_exception(self):
        config = {'seconds': 2, 'slow': {'worker::contract': 10}}
        rust_gate.check_budget({'worker::contract': 3, 'fast::one': 1}, config, config)
        with self.assertRaisesRegex(RuntimeError, 'fast::one'):
            rust_gate.check_budget({'fast::one': 3}, config, config)
        with self.assertRaisesRegex(RuntimeError, 'worker::contract'):
            rust_gate.check_budget({'worker::contract': 11}, config, config)

    def test_slow_list_and_budgets_can_only_shrink(self):
        baseline = {'seconds': 2, 'slow': {'contract': 10, 'removed': 20}}
        rust_gate.check_budget({}, {'seconds': 1, 'slow': {'contract': 8}}, baseline)
        for config in [
            {'seconds': 3, 'slow': {}},
            {'seconds': 2, 'slow': {'contract': 11}},
            {'seconds': 2, 'slow': {'new': 10}},
        ]:
            with self.subTest(config=config), self.assertRaisesRegex(RuntimeError, 'grow'):
                rust_gate.check_budget({}, config, baseline)

    def test_libtest_reports_actual_test_times_and_failed_exits_fail(self):
        with tempfile.TemporaryDirectory() as temporary:
            executable = Path(temporary) / 'fixture.py'
            executable.write_text(
                'import json, sys\n'
                'print(json.dumps({"type":"test", "event":"ok", "name":"one", "exec_time":0.25}))\n'
                'print(json.dumps({"type":"suite", "event":"ok", "passed":1, "failed":0, "ignored":0}))\n'
                'sys.exit(1 if "fail" in sys.argv else 0)\n'
            )
            binary = {'name': 'fixture', 'command': [sys.executable, str(executable)]}
            results = rust_gate.execute_binary(binary, custom=False)
            self.assertEqual(results, {'fixture::one': 0.25})
            with self.assertRaisesRegex(RuntimeError, 'exit'):
                rust_gate.execute_binary(binary, custom=False, arguments=('fail',))

    def test_custom_harness_measures_intervals_and_includes_unreported_tail(self):
        with tempfile.TemporaryDirectory() as temporary:
            executable = Path(temporary) / 'fixture.py'
            executable.write_text('print("test named_scenario ... ok")\n')
            binary = {'name': 'fixture', 'command': [sys.executable, str(executable)]}
            results = rust_gate.execute_binary(binary, custom=True)
            self.assertIn('fixture::named_scenario', results)
            self.assertIn('fixture::__tail__', results)
            self.assertGreaterEqual(results['fixture::named_scenario'], 0)

    def test_different_binaries_overlap_and_worker_count_is_bounded(self):
        barrier = threading.Barrier(2, timeout=5)
        active = 0
        peak = 0
        lock = threading.Lock()

        def execute(binary, custom):
            nonlocal active, peak
            with lock:
                active += 1
                peak = max(peak, active)
            barrier.wait()
            with lock:
                active -= 1
            return {binary['name']: 0}

        binaries = [{'name': str(i), 'custom': False} for i in range(4)]
        with patch.object(rust_gate, 'execute_binary', side_effect=execute):
            results = rust_gate.execute_binaries(binaries, jobs=2)
        self.assertEqual(peak, 2)
        self.assertEqual(set(results), {'0', '1', '2', '3'})

    def test_missing_native_results_and_skipped_tests_fail(self):
        for content in [
            'print("unrelated output")',
            'print(\'{"type":"test","event":"ignored","name":"missing"}\')',
        ]:
            with tempfile.TemporaryDirectory() as temporary:
                path = Path(temporary) / 'fixture.py'
                path.write_text(content)
                with self.assertRaisesRegex(RuntimeError, 'suite|skipped'):
                    rust_gate.execute_binary(
                        {'name': 'fixture', 'command': [sys.executable, str(path)]}, custom=False,
                    )


if __name__ == '__main__':
    unittest.main()
