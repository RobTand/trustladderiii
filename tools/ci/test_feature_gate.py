import unittest

import feature_gate


class FeatureGateTests(unittest.TestCase):
    def metadata(self):
        return {'workspace_members': ['kernel', 'controller'], 'packages': [
            {'id': 'kernel', 'name': 'tl3-kernel', 'features': {'serde': []}},
            {'id': 'controller', 'name': 'tl3-controller', 'features': {'loom': [], 'extra': []}},
            {'id': 'dependency', 'name': 'dependency', 'features': {'ignored': []}},
        ]}

    def test_every_workspace_feature_combination_builds_without_running_tests(self):
        commands = feature_gate.commands(self.metadata(), 'features')
        combinations = {}
        for command in commands:
            self.assertEqual(command[:2], ['cargo', 'test'])
            self.assertIn('--no-run', command)
            self.assertIn('--locked', command)
            self.assertIn('--no-default-features', command)
            package = command[command.index('-p') + 1]
            features = command[command.index('--features') + 1] if '--features' in command else ''
            combinations.setdefault(package, []).append(features)
            self.assertIn('--lib' if 'loom' in features.split(',') else '--all-targets', command)
        self.assertEqual({name: set(features) for name, features in combinations.items()}, {
            'tl3-kernel': {'', 'serde'}, 'tl3-controller': {'', 'extra', 'loom', 'extra,loom'},
        })
        self.assertEqual(len(commands), 6)

    def test_kani_only_compiles_every_kernel_feature_combination(self):
        commands = feature_gate.commands(self.metadata(), 'kani')
        self.assertEqual(len(commands), 2)
        for command in commands:
            self.assertEqual(command[:2], ['cargo', 'kani'])
            self.assertIn('--only-codegen', command)
            self.assertIn('--no-default-features', command)
            self.assertEqual(command[command.index('-p') + 1], 'tl3-kernel')
        self.assertEqual(sum('--features' in command for command in commands), 1)

    def test_unknown_compile_mode_fails_closed(self):
        with self.assertRaises(ValueError):
            feature_gate.commands(self.metadata(), 'unknown')


if __name__ == '__main__':
    unittest.main()
