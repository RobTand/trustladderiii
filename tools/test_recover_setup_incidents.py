import unittest

import recover_setup_incidents as recovery


def status_line(incident, summary=None):
    summary = summary or ('For "An incoming update", GitHub could not complete its setup. '
                          'Work is waiting for recovery. Other requests can continue.')
    return f'{summary} (clear: clear incident {incident})'


class RecoveryTests(unittest.TestCase):
    def test_only_the_diagnosed_open_incidents_are_planned(self):
        status = '\n'.join([
            status_line('71939.input.0'), status_line('88436.input.0'),
            status_line('99999.input.0'), status_line('72807.0'),
        ])
        self.assertEqual(recovery.plan(status), ['71939.input.0', '88436.input.0'])

    def test_changed_summary_aborts_before_any_clear(self):
        with self.assertRaises(SystemExit):
            recovery.plan(status_line('71939.input.0', 'A different problem.'))

    def test_already_cleared_incidents_are_skipped(self):
        self.assertEqual(recovery.plan('No open incidents.'), [])


if __name__ == '__main__':
    unittest.main()
