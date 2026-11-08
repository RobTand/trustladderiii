"""Hermetic fake-native controls. These tests never invoke a real Flux client."""
import ast
import copy
import fcntl
import hashlib
import importlib.util
import json
import os
import re
import stat
import subprocess
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('flux_deployment', Path(__file__).with_name('flux-deploy.py'))
fx = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fx)


class FluxAdmissionTests(unittest.TestCase):
    def setUp(self):
        previous = os.umask(0o077)
        self.addCleanup(os.umask, previous)
        temporary = tempfile.TemporaryDirectory(prefix='tl3-flux-fixture-', dir='/tmp')
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        lock_temporary = tempfile.TemporaryDirectory(prefix='tl3-owner-lock-fixture-', dir='/tmp')
        self.addCleanup(lock_temporary.cleanup)
        self.lock_root = Path(lock_temporary.name) / 'deploy'
        self.addCleanup(patch.stopall)
        patch.object(fx.pwd, 'getpwnam', return_value=SimpleNamespace(pw_uid=os.getuid())).start()
        patch.object(fx, 'LOCK_ROOT', self.lock_root, create=True).start()
        self.out = self.root / 'dispatch' / 'build'
        self.out.parent.mkdir()
        self.source = self.root / 'source'
        self.source.mkdir()
        self.secret = 'DISPOSABLE_MARKER_7b4299c0'
        patch.object(fx, 'SOURCE', self.source).start()
        patch.object(fx, 'SCRATCH', self.root / 'scratch').start()
        fx.SCRATCH.mkdir()
        patch.object(fx, 'CGROUP_ROOT', self.root / 'cgroups').start()
        fx.CGROUP_ROOT.mkdir()
        patch.dict(os.environ, {'TL3_PLANTED_MARKER': self.secret}).start()
        self.intent = fx.plan('a' * 40, 'b' * 40, 'build', 'dev', self.root / 'release',
                              self.root / 'private-manifest', self.root / 'private-backup', self.out)
        self.identifier = 'ƒ123abc'
        self.calls = []
        self.spec = None
        self.matches = 1
        self.lost_reply = False
        self.terminal_failure = False
        self.command_failure = 0
        self.bad_result = False
        self.bad_status = False
        self.bad_original = False
        self.disconnect = False
        self.watch_timeout = False
        self.drift = False
        patch.object(fx, 'native', self.native).start()

    def native(self, *args, **kwargs):
        self.calls.append(args)
        if args[0] == 'getattr':
            value = ('different' if self.drift else fx.STARTTIME) if args[1] == 'broker.starttime' else '3'
            return 0, value.encode(), b''
        if args[:2] == ('config', 'get'):
            return 0, fx.encoded({'exec': {'service': 'sdexec', 'sdexec-constrain-resources': False,
                    'sdexec-properties': {'MemoryMax': '75%', 'MemorySwapMax': '0', 'Slice': 'flux-jobs.slice'}}}), b''
        if args[:2] == ('submit', '--dry-run'):
            self.assertEqual(args[2], '--env=-*')
            self.assertIn('-n1', args)
            self.assertNotIn('-N1', args)
            self.assertIn('-t5400s', args)
            self.assertIn('--requires=host:dl380g10', args)
            env = dict(item[len('--env='):].split('=', 1) for item in args[3:] if item.startswith('--env='))
            self.assertEqual(env, self.intent['environment'])
            self.assertNotIn('TL3_PLANTED_MARKER', env)
            command = list(args[args.index('/usr/bin/python3'):])
            return 0, fx.encoded({'version': 1, 'resources': [{'type': 'slot', 'count': 1, 'label': 'task',
                    'with': [{'type': 'core', 'count': self.intent['cpus']}]}],
                    'tasks': [{'command': command, 'slot': 'task', 'count': {'per_slot': 1}}],
                    'attributes': {'system': {'environment': env, 'cwd': str(self.source), 'duration': 5400,
                    'constraints': {'hostlist': ['dl380g10']}}}}), b''
        if args[:2] == ('job', 'submit'):
            self.spec = fx.load(Path(args[2]))
            expected = fx.digest(fx.encoded(self.intent))
            for name in ('stdout', 'stderr'):
                fx.save(self.out / name, b'fixture output\n')
            fx.save(self.out / 'result.json', {'schema': 'tl3.deploy.payload.v1', 'intent': expected,
                    'job_id': self.identifier, 'returncode': self.command_failure, 'source_tree': self.intent['tree'],
                    'scratch_removed': True, 'cgroup': '/fixture-absent/flux-jobs.slice/shell-fixture:sys.service',
                    'cgroup_inode': 123})
            if self.lost_reply:
                return 74, b'', b'fixture lost submit reply\n'
            return 0, (self.identifier + '\n').encode(), b''
        if args[:2] == ('jobs', '-a'):
            return 0, fx.encoded({'jobs': [{'jobid': self.identifier, 'name': self.intent['name']}
                                          for _ in range(self.matches)]}), b''
        if args[:3] == ('job', 'info', '--original'):
            value = copy.deepcopy(self.spec)
            if self.bad_original:
                value['attributes']['system']['cwd'] = '/contradiction'
            return 0, fx.encoded(value), b''
        if args[:1] == ('cancel',):
            self.assertEqual(args, ('cancel', self.identifier))
            return 0, b'', b''
        if args[:2] == ('job', 'wait-event'):
            if self.watch_timeout:
                return 124, b'', b'fixture watch timeout\n'
            if self.disconnect:
                return 74, b'', b'fixture disconnect\n'
            if self.bad_result:
                fx.save(self.out / 'result.json', {'schema': 'malformed'})
            return 0, fx.encoded({'name': 'clean'}), b''
        if args[:2] == ('job', 'status'):
            value = {} if self.bad_status else {'id': 42, 'exception_occurred': False,
                    'waitstatus': self.command_failure << 8, 'result': 2 if self.command_failure else 1}
            return (1 if self.terminal_failure else self.command_failure), fx.encoded(value), b''
        if args[:2] == ('job', 'eventlog'):
            value = [{'name': 'start'}, {'name': 'finish', 'context': {'status': self.command_failure << 8}}, {'name': 'clean'}]
            if self.terminal_failure:
                value.insert(1, {'name': 'exception', 'context': {'type': 'exec', 'severity': 0}})
            return 0, b'\n'.join(fx.encoded(row) for row in value), b''
        if args[:2] == ('job', 'id'):
            return 0, b'42', b''
        self.fail('unexpected native fixture command')

    def submissions(self):
        return sum(call[:2] == ('job', 'submit') for call in self.calls)

    def test_owner_preflight_rejects_old_6900_second_allowance(self):
        module_spec = importlib.util.spec_from_file_location('budget_deployment', Path(__file__).with_name('deploy.py'))
        deploy = importlib.util.module_from_spec(module_spec)
        module_spec.loader.exec_module(deploy)
        # 5000s backup estimate alone leaves less than the required explicit
        # deployment margin and 90s stability inside a 5400s job.
        total = (5000 - 1800) * 20 * 1024 * 1024 // 8
        item = SimpleNamespace(lstat=lambda total=total: SimpleNamespace(st_mode=stat.S_IFREG, st_size=total))
        source = SimpleNamespace(rglob=lambda pattern, item=item: [item])
        with patch.object(deploy, 'validate_backup_destination'), \
             self.assertRaises(RuntimeError):
            deploy.preflight_backup({'data': source}, self.root / 'backup')

    def test_oversize_preflight_refuses_before_any_new_submission(self):
        module_spec = importlib.util.spec_from_file_location('budget_deployment', Path(__file__).with_name('deploy.py'))
        deploy = importlib.util.module_from_spec(module_spec)
        module_spec.loader.exec_module(deploy)
        total = (5000 - 1800) * 20 * 1024 * 1024 // 8
        item = SimpleNamespace(lstat=lambda total=total: SimpleNamespace(st_mode=stat.S_IFREG, st_size=total))
        source = SimpleNamespace(rglob=lambda pattern, item=item: [item])
        def checked(command, **kwargs):
            if 'preflight-fresh-budget' in command:
                deploy.preflight_backup({'data': source}, self.root / 'backup')
            return b''
        with patch.object(deploy, 'validate_backup_destination'), \
             patch.object(fx, 'STORE', self.root / 'evidence'), \
             patch.object(fx, 'source_identity', return_value='b' * 40), \
             patch.object(fx, 'checked', side_effect=checked), \
             patch.object(fx, 'admit_stages', return_value={}) as admitted, \
             patch.object(fx.sys, 'argv', ['flux-deploy.py', 'admit', 'a' * 40, 'dev',
                                         '/fixture/manifest', '/fixture/backup']):
            with self.assertRaises(RuntimeError):
                fx.main()
            admitted.assert_not_called()
        self.assertEqual(self.submissions(), 0)

    def test_job_shell_python_invocations_are_isolated(self):
        script = Path(__file__).with_name('build-release.sh').read_text()
        invocations = re.findall(r'(?m)^.*?\b(?:/usr/bin/)?python3\s+([^\n]+)', script)
        self.assertTrue(invocations)
        for arguments in invocations:
            self.assertRegex(arguments, r'^(?:-I|-s)(?:\s|$)')

    def test_backup_budget_boundary_keeps_stability_and_margin_inside_job(self):
        module_spec = importlib.util.spec_from_file_location('budget_deployment', Path(__file__).with_name('deploy.py'))
        deploy = importlib.util.module_from_spec(module_spec)
        module_spec.loader.exec_module(deploy)
        self.assertEqual(deploy.FRESH_BACKUP_BUDGET_S, 4800)
        self.assertEqual(deploy.FRESH_STABILITY_S, 90)
        self.assertEqual(deploy.FRESH_MARGIN_S, 510)
        self.assertEqual(deploy.FRESH_BACKUP_BUDGET_S + deploy.FRESH_STABILITY_S + deploy.FRESH_MARGIN_S, 5400)
        boundary = (4800 - 1800) * 20 * 1024 * 1024 // 8
        for total, refused in ((0, False), (boundary, False), (boundary + 1, True)):
            item = SimpleNamespace(lstat=lambda total=total: SimpleNamespace(st_mode=stat.S_IFREG, st_size=total))
            source = SimpleNamespace(rglob=lambda pattern, item=item: [item])
            with self.subTest(total=total), patch.object(deploy, 'validate_backup_destination'):
                if refused:
                    with self.assertRaises(RuntimeError):
                        deploy.preflight_backup({'data': source}, self.root / 'backup')
                else:
                    deploy.preflight_backup({'data': source}, self.root / 'backup')

    def test_pre_submit_inventory_is_metadata_only_and_counts_entire_runs_root(self):
        module_spec = importlib.util.spec_from_file_location('budget_deployment', Path(__file__).with_name('deploy.py'))
        deploy = importlib.util.module_from_spec(module_spec)
        module_spec.loader.exec_module(deploy)
        data, config, runs, tablespace = [self.root / name for name in ('data', 'config', 'runs', 'tablespace')]
        for directory in (data, config, runs, tablespace):
            directory.mkdir()
        config_file = config / 'tl3.toml'
        config_file.write_text('[workers]\nruns = "' + str(runs) + '"\n')
        manifest = self.root / 'manifest'
        manifest.write_bytes(fx.encoded({'schema': 'tl3.fresh-init.v1', 'head': 'a' * 40,
            'config_sha256': hashlib.sha256(config_file.read_bytes()).hexdigest(),
            'archive': {}, 'tablespaces': {'1234': str(tablespace)}}))
        args = SimpleNamespace(home=self.root, head='a' * 40, state_manifest=manifest, backup=self.root / 'backup')
        real_preflight = deploy.preflight_backup
        captured = []
        def preflight(sources, backup, **kwargs):
            captured.append((sources, kwargs))
            return real_preflight(sources, backup, **kwargs)
        # paths() expects root/postgres/data. Use a synthetic TL3 root.
        (self.root / 'postgres').mkdir()
        data.rename(self.root / 'postgres/data')
        with patch.object(deploy, 'paths', return_value=(self.root, config, self.root / 'pg-bin')), \
             patch.object(deploy, 'preflight_backup', side_effect=preflight), \
             patch.object(deploy, 'tree_manifest') as content_hash:
            deploy.preflight_fresh_budget(args)
            content_hash.assert_not_called()
        self.assertEqual(captured[0][0]['runs'], runs)
        self.assertEqual(captured[0][0]['tablespace-1234'], tablespace)
        self.assertEqual(captured[0][1], {'metadata_only': True})
        self.assertFalse(args.backup.exists())

    def test_python_command_lists_in_job_call_chain_are_isolated(self):
        for name in ('flux-deploy.py', 'deploy.py'):
            tree = ast.parse(Path(__file__).with_name(name).read_text())
            for node in ast.walk(tree):
                if not isinstance(node, ast.List) or not node.elts:
                    continue
                first = node.elts[0]
                python = (isinstance(first, ast.Constant) and isinstance(first.value, str)
                          and Path(first.value).name in ('python', 'python3'))
                python = python or (isinstance(first, ast.Attribute) and isinstance(first.value, ast.Name)
                                    and first.value.id == 'sys' and first.attr == 'executable')
                if python:
                    with self.subTest(file=name, line=node.lineno):
                        self.assertGreater(len(node.elts), 1)
                        self.assertIsInstance(node.elts[1], ast.Constant)
                        self.assertIn(node.elts[1].value, ('-I', '-s'))

    def test_reattach_submitted_fresh_job_skips_new_backup_preflight(self):
        fresh = self.out.parent / 'fresh-init'
        fresh.mkdir()
        fx.save(fresh / 'dispatched.json', {})
        calls = []
        def checked(command, **kwargs):
            calls.append(command)
            if 'preflight-fresh-budget' in command:
                raise RuntimeError('a submitted job may already have created its backup')
            return b''
        with patch.object(fx, 'STORE', self.root), \
             patch.object(fx, 'source_identity', return_value='b' * 40), \
             patch.object(fx, 'checked', side_effect=checked), \
             patch.object(fx, 'admit_stages', return_value={}) as admitted, \
             patch.object(fx.sys, 'argv', ['flux-deploy.py', 'admit', 'a' * 40, 'dev',
                                         '/fixture/manifest', '/fixture/backup', '--reattach', str(self.out.parent)]):
            fx.main()
            admitted.assert_called_once()
        self.assertEqual(len(calls), 1)
        self.assertIn('qualify-fresh', calls[0])
        self.assertEqual(self.submissions(), 0)

    def test_source_path_does_not_select_lock_or_evidence_root(self):
        self.assertEqual(fx.STORE, Path('/home/rob/.local/share/tl3/deploy-admission'))
        with patch.object(fx, 'SOURCE', self.root / 'another-source'):
            self.assertEqual(fx.STORE, Path('/home/rob/.local/share/tl3/deploy-admission'))
            self.assertEqual(fx.LOCK_ROOT, self.lock_root)

    def test_owner_walltime_is_5400_seconds(self):
        self.assertEqual(self.intent['walltime_s'], 5400)

    def test_owner_0755_lock_root_contains_only_lock(self):
        shared = self.root / 'owner-lock'
        shared.mkdir(mode=0o755)
        shared.chmod(0o755)
        evidence = self.root / 'private-evidence'
        with patch.object(fx, 'LOCK_ROOT', shared, create=True), \
             patch.object(fx, 'admit_stages', return_value={}):
            self.coordinator(evidence)
        self.assertEqual(sorted(p.name for p in shared.iterdir()), ['deploy.lock'])
        self.assertEqual(shared.stat().st_mode & 0o777, 0o755)
        self.assertEqual(evidence.stat().st_mode & 0o777, 0o700)
        self.assertFalse((evidence / 'deploy.lock').exists())

    def test_owner_limit_verifier_checks_actual_jobspec_and_saved_intent(self):
        good = fx.jobspec(self.intent, fx.digest(fx.encoded(self.intent)))
        for field, maximum, leaf in (('cpus', 8, 0), ('memory_gib', 32, 1),
                                     ('walltime_s', 5400, None)):
            for value in (None, True, 0, -1, '8', float('nan'), float('inf'), maximum + 1):
                with self.subTest(field=field, value=value):
                    intent = copy.deepcopy(self.intent)
                    spec = copy.deepcopy(good)
                    intent[field] = value
                    if leaf is None:
                        spec['attributes']['system']['duration'] = value
                    else:
                        spec['resources'][0]['with'][leaf]['count'] = value
                    with self.assertRaises((RuntimeError, KeyError, TypeError, ValueError)):
                        fx.validate_spec(intent, fx.digest(fx.encoded(intent)), spec)
        for field, maximum, leaf in (('cpus', 8, 0), ('memory_gib', 32, 1),
                                     ('walltime_s', 5400, None)):
            intent = copy.deepcopy(self.intent)
            spec = copy.deepcopy(good)
            intent[field] = maximum
            if leaf is None:
                spec['attributes']['system']['duration'] = maximum
            else:
                spec['resources'][0]['with'][leaf]['count'] = maximum
            request = fx.digest(fx.encoded(intent))
            spec['tasks'][0]['command'][-1] = request
            spec['attributes']['user']['tl3_deploy_intent'] = request
            fx.validate_spec(intent, request, spec)
        self.assertEqual(self.submissions(), 0)

    def test_invalid_persisted_shapes_refuse_before_submit(self):
        good = fx.jobspec(self.intent, fx.digest(fx.encoded(self.intent)))
        mutations = [
            lambda s: s['attributes']['system'].pop('constraints'),
            lambda s: s['attributes']['system'].update(constraints={}),
            lambda s: s['attributes']['system'].update(constraints=None),
            lambda s: s['attributes']['system'].update(constraints={'hostlist': 'dl380g10'}),
            lambda s: s['attributes']['system'].update(constraints={'hostlist': ['sparky']}),
            lambda s: s['attributes']['system'].update(constraints={'hostlist': ['dl380g10', 'sparky']}),
            lambda s: s['attributes']['system'].update(constraints={'or': [{'hostlist': ['dl380g10']}, {}]}),
            lambda s: s['attributes']['system'].update(constraints={'hostlist': ['dl380g10'], 'properties': []}),
            lambda s: s['attributes']['system'].pop('duration'),
            lambda s: s['resources'][0].update(exclusive=True),
            lambda s: s['attributes']['system'].update(exclusive=True),
            lambda s: s['resources'][0].update(label=[]),
            lambda s: s['resources'][0].update(type='node'),
            lambda s: s['resources'][0].update(count=2),
            lambda s: s['resources'][0]['with'].pop(),
            lambda s: s['resources'][0]['with'][0].pop('count'),
            lambda s: s['resources'][0]['with'][0].update(count=9),
            lambda s: s['resources'][0]['with'][1].update(count=33),
            lambda s: s['attributes']['system'].update(duration=5401),
        ]
        self.out.mkdir()
        for change in mutations:
            spec = copy.deepcopy(good)
            change(spec)
            fx.save(self.out / 'jobspec.json', spec)
            with self.subTest(change=change), self.assertRaises((RuntimeError, KeyError, TypeError)):
                fx.stage(self.intent)
            self.assertFalse((self.out / 'dispatched.json').exists())
        self.assertEqual(self.submissions(), 0)

    def test_invalid_dry_run_host_or_shape_refuses_before_submit(self):
        original = self.native
        for field, value in (('constraints', {}), ('constraints', {'hostlist': ['sparky']}),
                             ('duration', 7200)):
            def altered(*args, field=field, value=value, **kwargs):
                rc, raw, error = original(*args, **kwargs)
                if args[:2] == ('submit', '--dry-run'):
                    spec = json.loads(raw)
                    spec['attributes']['system'][field] = value
                    raw = fx.encoded(spec)
                return rc, raw, error
            with patch.object(fx, 'native', side_effect=altered), \
                 self.subTest(field=field, value=value), self.assertRaises(RuntimeError):
                fx.stage(self.intent)
            self.assertFalse((self.out / 'dispatched.json').exists())
        self.assertEqual(self.submissions(), 0)

    def test_lock_layout_refuses_invalid_mode_symlink_owner_and_foreign_entry(self):
        self.lock_root.mkdir()
        for mode in (0o700, 0o775, 0o777):
            self.lock_root.chmod(mode)
            with self.subTest(mode=mode), self.assertRaises(RuntimeError):
                self.coordinator(self.root / 'evidence')
            self.assertEqual(self.lock_root.stat().st_mode & 0o777, mode)
        self.lock_root.chmod(0o755)
        foreign = self.lock_root / 'old-uuid'
        foreign.mkdir()
        with self.assertRaises(RuntimeError):
            self.coordinator(self.root / 'evidence')
        foreign.rmdir()
        target = self.root / 'target'
        target.mkdir()
        self.lock_root.rmdir()
        self.lock_root.symlink_to(target)
        with self.assertRaises(RuntimeError):
            self.coordinator(self.root / 'evidence')
        self.lock_root.unlink()
        self.lock_root.mkdir(mode=0o755)
        self.lock_root.chmod(0o755)
        real_lstat = Path.lstat
        def wrong_owner(path, *args, **kwargs):
            info = real_lstat(path, *args, **kwargs)
            if path == self.lock_root:
                return SimpleNamespace(st_mode=info.st_mode, st_uid=info.st_uid + 1)
            return info
        with patch.object(Path, 'lstat', wrong_owner), self.assertRaises(RuntimeError):
            self.coordinator(self.root / 'evidence')
        self.assertEqual(self.submissions(), 0)

    def test_lock_file_symlink_unowned_nonregular_and_hardlink_refuse(self):
        self.lock_root.mkdir(mode=0o755)
        self.lock_root.chmod(0o755)
        lock = self.lock_root / 'deploy.lock'
        target = self.root / 'target'
        target.write_text('do not overwrite')
        lock.symlink_to(target)
        with self.assertRaises((OSError, RuntimeError)):
            self.coordinator(self.root / 'evidence')
        lock.unlink()
        lock.mkdir()
        with self.assertRaises((OSError, RuntimeError)):
            self.coordinator(self.root / 'evidence')
        lock.rmdir()
        os.link(target, lock)
        with self.assertRaises(RuntimeError):
            self.coordinator(self.root / 'evidence')
        self.assertEqual(target.read_text(), 'do not overwrite')
        lock.unlink()
        lock.write_text('unchanged')
        real_lstat = Path.lstat
        def wrong_owner(path, *args, **kwargs):
            info = real_lstat(path, *args, **kwargs)
            if path == lock:
                return SimpleNamespace(st_mode=info.st_mode, st_uid=info.st_uid + 1)
            return info
        with patch.object(Path, 'lstat', wrong_owner), self.assertRaises(RuntimeError):
            self.coordinator(self.root / 'evidence')
        self.assertEqual(lock.read_text(), 'unchanged')
        self.assertEqual(self.submissions(), 0)

    def test_evidence_store_invalid_mode_symlink_and_ancestry_refuse(self):
        store = self.root / 'evidence'
        store.mkdir()
        store.chmod(0o755)
        with self.assertRaises(RuntimeError):
            self.coordinator(store)
        self.assertEqual(store.stat().st_mode & 0o777, 0o755)
        store.rmdir()
        store.symlink_to(self.out.parent)
        with self.assertRaises(RuntimeError):
            self.coordinator(store)
        store.unlink()
        ancestor = self.root / 'alias'
        ancestor.symlink_to(self.out.parent)
        with self.assertRaises(RuntimeError):
            self.coordinator(ancestor / 'must-not-create')
        self.assertFalse((self.out.parent / 'must-not-create').exists())
        with patch.object(fx, 'LOCK_ROOT', ancestor / 'lock'), self.assertRaises(RuntimeError):
            self.coordinator(store)
        self.assertFalse((self.out.parent / 'lock').exists())
        self.assertEqual(self.submissions(), 0)

    def test_global_lock_remains_held_through_accepted_write(self):
        real_save = fx.save
        observed = []
        def save(path, value):
            if path.name == 'accepted.json':
                with (self.lock_root / 'deploy.lock').open('a') as contender, \
                     self.assertRaises(BlockingIOError):
                    fcntl.flock(contender, fcntl.LOCK_EX | fcntl.LOCK_NB)
                observed.append(path)
            return real_save(path, value)
        with patch.object(fx, 'save', side_effect=save), \
             patch.object(fx, 'admit_stages', return_value={}):
            self.coordinator(self.root / 'evidence')
        self.assertEqual(len(observed), 1)

    def test_different_evidence_stores_share_same_physical_global_lock(self):
        store = self.root / 'first-evidence'
        store.mkdir()
        with patch.object(fx, 'STORE', store), fx.serialized_admission(None), \
             patch.object(fx, 'admit_stages', return_value={}) as admitted:
            with self.assertRaises(BlockingIOError):
                self.coordinator(self.root / 'other-evidence')
            admitted.assert_not_called()
        self.assertEqual(list(self.lock_root.iterdir()), [self.lock_root / 'deploy.lock'])

    def assert_private_clean_evidence(self):
        for path in self.out.rglob('*'):
            if path.is_file():
                self.assertNotIn(self.secret.encode(), path.read_bytes())
                self.assertEqual(path.stat().st_mode & 0o077, 0)

    def test_success_receipt_memory_environment_and_binding(self):
        fx.binding()
        receipt_digest = fx.stage(self.intent)
        self.assertEqual(self.submissions(), 1)
        receipt = fx.load(self.out / 'receipt.json')
        self.assertEqual(fx.digest(fx.encoded(receipt)), receipt_digest)
        self.assertEqual(receipt['generation'], fx.GENERATION)
        self.assertEqual(fx.load(self.out / 'binding.json')['job_id'], self.identifier)
        slot = self.spec['resources'][0]
        self.assertEqual(slot['with'], [{'type': 'core', 'count': 8}, {'type': 'memory', 'count': 8}])
        self.assertEqual(self.spec['attributes']['system']['environment']['CARGO_BUILD_JOBS'], '2')
        self.assertEqual(self.spec['attributes']['user']['source_head'], 'a' * 40)
        self.assertEqual(self.spec['attributes']['system']['duration'], 5400)
        self.assertIn('--requires=host:dl380g10', self.calls[3])
        self.assert_private_clean_evidence()
        for name, reference in receipt['files'].items():
            self.assertEqual(hashlib.sha256((self.out / name).read_bytes()).hexdigest(), reference['sha256'])

    def test_fresh_is_cpu1_memory2_and_only_required_environment(self):
        self.intent = fx.plan('a' * 40, 'b' * 40, 'fresh-init', 'dev', self.root / 'release',
                              self.root / 'manifest', self.root / 'backup', self.out)
        fx.stage(self.intent)
        self.assertEqual(self.spec['resources'][0]['with'], [{'type': 'core', 'count': 1}, {'type': 'memory', 'count': 2}])
        environment = self.spec['attributes']['system']['environment']
        self.assertNotIn('CARGO_BUILD_JOBS', environment)
        self.assertEqual(environment['XDG_RUNTIME_DIR'], '/run/user/1000')
        self.assertTrue(all(environment[key] == '1' for key in fx.THREADS))
        self.assert_private_clean_evidence()

    def test_lost_reply_recovers_one_original_intent_without_resubmit(self):
        self.lost_reply = True
        fx.stage(self.intent)
        fx.stage(self.intent)
        self.assertEqual(self.submissions(), 1)

    def test_zero_multiple_and_contradictory_matches_remain_unknown(self):
        self.lost_reply = True
        for matches, contradiction in ((0, False), (2, False), (1, True)):
            with self.subTest(matches=matches, contradiction=contradiction):
                self.matches, self.bad_original = matches, contradiction
                with self.assertRaises(RuntimeError):
                    fx.stage(self.intent)
                self.assertFalse((self.out / 'receipt.json').exists())
        self.assertEqual(self.submissions(), 1)

    def test_disconnect_reattaches_same_id_and_collects_terminal_streams(self):
        self.disconnect = True
        with self.assertRaises(RuntimeError):
            fx.stage(self.intent)
        self.assertTrue((self.out / 'status.json').exists())
        self.assertTrue((self.out / 'eventlog.jsonl').exists())
        self.disconnect = False
        fx.stage(self.intent)
        self.assertEqual(self.submissions(), 1)
        self.assertEqual(sum(call[:2] == ('job', 'wait-event') for call in self.calls), 2)

    def test_watch_timeout_cancels_only_owned_id_and_collects_failure(self):
        self.watch_timeout = True
        with self.assertRaises(RuntimeError):
            fx.stage(self.intent)
        self.assertEqual([call for call in self.calls if call[:1] == ('cancel',)], [('cancel', self.identifier)])
        self.assertTrue((self.out / 'status.json').exists())
        self.assertTrue((self.out / 'eventlog.jsonl').exists())
        self.assertTrue((self.out / 'refusal.json').exists())
        self.assertFalse((self.out / 'receipt.json').exists())

    def test_terminal_failure_cannot_be_overridden_by_payload_success(self):
        self.terminal_failure = True
        with self.assertRaises(RuntimeError):
            fx.stage(self.intent)
        self.assertTrue((self.out / 'eventlog.jsonl').exists())
        self.assertFalse((self.out / 'receipt.json').exists())

    def test_malformed_status_or_payload_refuses(self):
        for attribute in ('bad_status', 'bad_result'):
            with self.subTest(attribute=attribute):
                setattr(self, attribute, True)
                with self.assertRaises((RuntimeError, KeyError)):
                    fx.stage(self.intent)
                setattr(self, attribute, False)
        self.assertEqual(self.submissions(), 1)

    def test_unknown_generation_refuses_before_admission(self):
        self.drift = True
        with self.assertRaises(RuntimeError):
            fx.binding()
        self.assertEqual(self.submissions(), 0)

    def test_missing_or_malformed_id_refuses(self):
        for raw in (b'', b'123', b'foo bar', '\nƒabc\nƒdef'.encode()):
            with self.assertRaises(RuntimeError):
                fx.job_id(raw)

    def test_reattach_intent_drift_refuses_without_resubmit(self):
        fx.stage(self.intent)
        self.intent['head'] = 'c' * 40
        with self.assertRaises(RuntimeError):
            fx.stage(self.intent)
        self.assertEqual(self.submissions(), 1)

    def test_missing_required_cleanup_identity_refuses_even_after_scope_removal(self):
        fx.stage(self.intent)
        result = fx.load(self.out / 'result.json')
        for field in ('cgroup_inode', 'intent', 'source_tree', 'scratch_removed'):
            altered = dict(result)
            altered.pop(field)
            fx.save(self.out / 'result.json', altered)
            with self.subTest(field=field), self.assertRaises(RuntimeError):
                fx.stage(self.intent)
        self.assertEqual(self.submissions(), 1)

    def test_missing_cleanup_receipt_cannot_pass_on_clean(self):
        fx.stage(self.intent)
        (self.out / 'result.json').unlink()
        (self.out / 'receipt.json').unlink()
        with self.assertRaises(FileNotFoundError):
            fx.stage(self.intent)
        self.assertEqual(self.submissions(), 1)

    def test_new_fresh_attempt_reuses_only_verified_prior_build_with_new_backup(self):
        fx.stage(self.intent)
        release = Path(self.intent['release'])
        release.mkdir()
        cycle = self.root / 'new-cycle'
        cycle.mkdir()
        real_stage = fx.stage
        fresh = []
        def execute(intent):
            if intent['stage'] == 'build':
                return real_stage(intent)
            fresh.append(intent)
            return 'c' * 64
        with patch.object(fx, 'STORE', self.root), patch.object(fx, 'stage', side_effect=execute):
            fx.admit_stages('a' * 40, 'b' * 40, 'dev', release, self.root / 'new-manifest',
                            self.root / 'NEW-backup', cycle, self.out.parent)
            fx.admit_stages('a' * 40, 'b' * 40, 'dev', release, self.root / 'new-manifest',
                            self.root / 'NEW-backup', cycle)
        self.assertEqual(self.submissions(), 1)
        self.assertEqual(fresh[0]['backup'], str(self.root / 'NEW-backup'))
        self.assertEqual(fresh[0]['manifest'], str(self.root / 'new-manifest'))
        self.assertNotEqual(fresh[0]['out'], str(self.out))
        self.assertTrue((cycle / 'reused-build.json').exists())

    def test_existing_release_without_original_build_binding_refuses(self):
        release = Path(self.intent['release'])
        release.mkdir()
        with patch.object(fx, 'stage') as execute:
            with self.assertRaises(RuntimeError):
                fx.admit_stages('a' * 40, 'b' * 40, 'dev', release, self.root / 'manifest',
                                self.root / 'backup', self.out.parent)
        execute.assert_not_called()
        self.assertEqual(self.submissions(), 0)

    def test_populated_scope_and_changed_identity_refuse_despite_native_clean(self):
        fx.stage(self.intent)
        (self.out / 'receipt.json').unlink()
        cgroup_root = self.root / 'cgroups'
        scope = cgroup_root / 'fixture-absent/flux-jobs.slice/shell-fixture:sys.service'
        scope.mkdir(parents=True)
        result = fx.load(self.out / 'result.json')
        result['cgroup_inode'] = scope.stat().st_ino
        fx.save(self.out / 'result.json', result)
        (scope / 'cgroup.events').write_text('populated 1\n')
        with patch.object(fx, 'CGROUP_ROOT', cgroup_root):
            with self.assertRaises(RuntimeError):
                fx.stage(self.intent)
            (scope / 'cgroup.events').write_text('populated 0\n')
            fx.stage(self.intent)
            result['cgroup_inode'] += 1
            fx.save(self.out / 'result.json', result)
            with self.assertRaises(RuntimeError):
                fx.stage(self.intent)
        self.assertEqual(self.submissions(), 1)

    def test_owned_scratch_remaining_refuses(self):
        fx.stage(self.intent)
        (fx.SCRATCH / self.identifier).mkdir()
        with self.assertRaises(RuntimeError):
            fx.stage(self.intent)
        self.assertEqual(self.submissions(), 1)

    def test_direct_admission_never_submits_after_unqualified_gate(self):
        with patch.object(fx, 'source_identity', return_value='b' * 40), \
             patch.object(fx, 'checked', side_effect=RuntimeError('fixture qualification refusal')), \
             patch.object(fx.sys, 'argv', ['flux-deploy.py', 'admit', 'a' * 40, 'dev', '/fixture/manifest', '/fixture/backup']):
            with self.assertRaises(RuntimeError):
                fx.main()
        self.assertEqual(self.submissions(), 0)

    def coordinator(self, store, *extra):
        with patch.object(fx, 'STORE', store), \
             patch.object(fx, 'source_identity', return_value='b' * 40), \
             patch.object(fx, 'checked', return_value=b''), \
             patch.object(fx.sys, 'argv', ['flux-deploy.py', 'admit', 'a' * 40, 'dev',
                                         '/fixture/manifest', '/fixture/backup', *extra]):
            fx.main()

    def test_store_lock_excludes_a_second_coordinator(self):
        store = self.root / 'store'
        store.mkdir()
        self.lock_root.mkdir(mode=0o755)
        self.lock_root.chmod(0o755)
        with (self.lock_root / 'deploy.lock').open('a') as lock, \
             patch.object(fx, 'admit_stages', return_value={}) as admitted:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            with self.assertRaises(BlockingIOError):
                self.coordinator(store)
            admitted.assert_not_called()
        self.assertEqual(self.submissions(), 0)

    def test_unresolved_sibling_blocks_new_cycle_even_with_refusal_or_build_reuse(self):
        for marker in ('dispatched.json', 'binding.json'):
            with self.subTest(marker=marker):
                store = self.root / marker
                old = store / 'old-cycle'
                stage = old / 'fresh-init'
                stage.mkdir(parents=True)
                fx.save(stage / marker, {})
                fx.save(stage / 'refusal.json', {'reason': 'transport_unknown'})
                fx.save(old / 'refusal.json', {'reason': 'transport_unknown'})
                fx.save(old / 'accepted.json', {})
                with patch.object(fx, 'admit_stages', return_value={}) as admitted:
                    with self.assertRaises(RuntimeError):
                        self.coordinator(store, '--build-dispatch', str(old))
                    admitted.assert_not_called()
                self.assertEqual([path.name for path in store.iterdir() if path.is_dir()], ['old-cycle'])
        self.assertEqual(self.submissions(), 0)

    def test_store_lock_spans_both_stages_and_is_released_afterward(self):
        store = self.root / 'store'
        def admitted(*args):
            with (self.lock_root / 'deploy.lock').open('a') as contender:
                with self.assertRaises(BlockingIOError):
                    fcntl.flock(contender, fcntl.LOCK_EX | fcntl.LOCK_NB)
            return {}
        with patch.object(fx, 'admit_stages', side_effect=admitted):
            self.coordinator(store)
        with (self.lock_root / 'deploy.lock').open('a') as contender:
            fcntl.flock(contender, fcntl.LOCK_EX | fcntl.LOCK_NB)
        self.assertEqual(self.submissions(), 0)

    def test_closed_failure_allows_new_cycle_without_becoming_a_pass(self):
        self.command_failure = 7
        with self.assertRaises(RuntimeError):
            fx.stage(self.intent)
        self.assertTrue((self.out / 'closed.json').exists())
        self.assertTrue((self.out / 'refusal.json').exists())
        self.assertFalse((self.out / 'receipt.json').exists())
        with patch.object(fx, 'admit_stages', return_value={}) as admitted:
            self.coordinator(self.root)
            admitted.assert_called_once()
        self.assertEqual(self.submissions(), 1)

    def test_unknown_cleanup_blocks_new_cycle_after_terminal_failure(self):
        self.command_failure = 7
        self.bad_result = True
        with self.assertRaises(RuntimeError):
            fx.stage(self.intent)
        self.assertFalse((self.out / 'closed.json').exists())
        with patch.object(fx, 'admit_stages', return_value={}) as admitted:
            with self.assertRaises(RuntimeError):
                self.coordinator(self.root)
            admitted.assert_not_called()
        self.assertEqual(self.submissions(), 1)

    def test_refusal_after_disconnect_blocks_new_cycle_but_allows_original_reattach(self):
        self.disconnect = True
        with self.assertRaises(RuntimeError):
            fx.stage(self.intent)
        self.assertFalse((self.out / 'closed.json').exists())
        with patch.object(fx, 'admit_stages', return_value={}) as admitted:
            with self.assertRaises(RuntimeError):
                self.coordinator(self.root)
            admitted.assert_not_called()
            self.coordinator(self.root, '--reattach', str(self.out.parent))
            admitted.assert_called_once()
        self.disconnect = False
        fx.stage(self.intent)
        self.assertEqual(self.submissions(), 1)

    def test_changed_closed_evidence_refuses_new_cycle(self):
        fx.stage(self.intent)
        fx.save(self.out / 'stdout', b'changed after collection')
        with patch.object(fx, 'admit_stages', return_value={}) as admitted:
            with self.assertRaises(RuntimeError):
                self.coordinator(self.root)
            admitted.assert_not_called()

    def test_reattach_cannot_bypass_a_different_unresolved_dispatch(self):
        other = self.root / 'other-cycle' / 'build'
        other.mkdir(parents=True)
        fx.save(other / 'dispatched.json', {})
        with patch.object(fx, 'admit_stages', return_value={}) as admitted:
            with self.assertRaises(RuntimeError):
                self.coordinator(self.root, '--reattach', str(self.out.parent))
            admitted.assert_not_called()
        self.assertEqual(self.submissions(), 0)

    def test_supervisor_environment_is_separate_from_caller(self):
        child = SimpleNamespace(pid=12345, returncode=0)
        child.communicate = lambda timeout=None: (b'fixture', b'')
        class Context:
            def __enter__(self):
                return child
            def __exit__(self, *args):
                return False
        with patch.object(fx.subprocess, 'Popen', return_value=Context()) as popen:
            self.assertEqual(fx.run(['/fake/native']), (0, b'fixture', b''))
        self.assertEqual(popen.call_args.kwargs['env'], fx.SUPERVISOR)
        self.assertNotIn('TL3_PLANTED_MARKER', popen.call_args.kwargs['env'])
        self.assertEqual(popen.call_args.kwargs['env']['FLUX_URI'], fx.URI)

    def test_payload_executes_direct_host_command_and_preserves_failure_cleanup(self):
        self.out.mkdir()
        original_read = Path.read_text
        original_stat = Path.stat
        cgroup = '/fixture/flux-jobs.slice/shell-fixture:sys.service'
        def read(path, *args, **kwargs):
            if str(path) == '/proc/self/cgroup':
                return '0::' + cgroup
            return original_read(path, *args, **kwargs)
        def info(path, *args, **kwargs):
            if str(path) == str(fx.CGROUP_ROOT) + cgroup:
                return SimpleNamespace(st_ino=456)
            return original_stat(path, *args, **kwargs)
        commands = []
        class Child:
            pid = 12345
            def __enter__(self):
                return self
            def __exit__(self, *args):
                return False
            def wait(self, timeout):
                return 7
        def spawn(command, **kwargs):
            commands.append((command, kwargs))
            return Child()
        def checked(command, **kwargs):
            return self.identifier.encode() if command[0] == fx.FLUX else b'btrfs'
        for stage in ('build', 'fresh-init'):
            self.intent = fx.plan('a' * 40, 'b' * 40, stage, 'dev', self.root / 'release',
                                  self.root / 'manifest', self.root / 'backup', self.out)
            intent_path = self.out / 'intent.json'
            fx.save(intent_path, self.intent)
            with patch.object(fx, 'source_identity', return_value='b' * 40), \
                 patch.object(fx.socket, 'gethostname', return_value='dl380g10'), \
                 patch.object(fx, 'checked', side_effect=checked), \
                 patch.object(Path, 'read_text', read), patch.object(Path, 'stat', info), \
                 patch.object(fx.subprocess, 'Popen', side_effect=spawn), \
                 patch.object(fx.os, 'killpg'), patch.dict(os.environ, {'FLUX_JOB_ID': '42'}):
                with self.assertRaises(RuntimeError):
                    fx.payload(intent_path, fx.digest(fx.encoded(self.intent)))
            result = fx.load(self.out / 'result.json')
            self.assertEqual(result['returncode'], 7)
            self.assertTrue(result['scratch_removed'])
            command, options = commands[-1]
            self.assertEqual(options['cwd'], self.source)
            self.assertNotIn('TL3_PLANTED_MARKER', options['env'])
            self.assertEqual(options['env']['XDG_RUNTIME_DIR'], '/run/user/1000')
            self.assertFalse(any('ssh' in value or 'systemd-run' in value for value in command))
            self.assertIn('build-release.sh' if stage == 'build' else 'deploy.py', command[1 if stage == 'build' else 2])
            self.assertFalse((fx.SCRATCH / self.identifier).exists())
        self.assert_private_clean_evidence()


class CallerFixtureTests(unittest.TestCase):
    def setUp(self):
        previous = os.umask(0o077)
        self.addCleanup(os.umask, previous)
        temporary = tempfile.TemporaryDirectory(prefix='tl3-flux-caller-')
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.repo = self.root / 'repo'
        self.repo.mkdir()
        self.directory = self.repo / 'deploy/dl380g10'
        self.directory.mkdir(parents=True)
        self.notes = self.root / 'notes'
        self.notes.mkdir()
        self.marker = self.root / 'native-dispatched'
        baseline = Path(__file__).with_name('fixtures') / 'deploy-main.pb-baseline.sh'
        self.assertEqual(hashlib.sha256(baseline.read_bytes()).hexdigest(),
                         '78d9a2c6aa7ff974afe21ca0ef98c937dea354c235cf818c8fa73ce8b932e402')
        selected = baseline if os.environ.get('TL3_FLUX_RED') == '1' else Path(__file__).with_name('deploy-main.sh')
        caller = selected.read_text().replace('notes=/home/rob/tmp/tl3-build', f'notes={self.notes}')
        caller = caller.replace('tools=/mnt/shared/prismabuild-fleet/repo/tools', f'tools={self.root}')
        (self.directory / 'deploy-main.sh').write_text(caller)
        (self.root / 'pbrun.py').write_text("raise SystemExit('fixture expects native Flux, saw replaced PB transport')\n")
        (self.directory / 'flux-deploy.py').write_text(f"from pathlib import Path\nPath({str(self.marker)!r}).write_text('native fixture only')\n")
        (self.directory / 'deploy.py').write_text("import os\nraise SystemExit(int(os.environ.get('TL3_FIXTURE_GATE', '0')))\n")
        self.environment = {key: value for key, value in os.environ.items() if not key.startswith('GIT_')}
        self.environment.update(TL3_FRESH_STATE_MANIFEST=str(self.root / 'manifest'),
                                TL3_COLD_BACKUP=str(self.root / 'backup'), TL3_DEPLOY_LANE='dev')
        self.git('init', '--quiet')
        self.git('add', '.')
        self.git('commit', '--quiet', '-m', 'generated fixture source')
        self.head = self.git('rev-parse', 'HEAD').decode().strip()

    def git(self, *args):
        return subprocess.check_output(['git', '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid',
                                        '-c', 'commit.gpgsign=false', *args], cwd=self.repo,
                                       env=self.environment, stderr=subprocess.PIPE, timeout=20)

    def call(self, *args, environment=None):
        return subprocess.run(['/bin/bash', str(self.directory / 'deploy-main.sh'), *args], cwd=self.root,
                              env=environment or self.environment, capture_output=True, timeout=40)

    def test_canonical_transport_is_native(self):
        result = self.call(self.head)
        self.assertEqual(result.returncode, 0, result.stderr.decode())
        self.assertTrue(self.marker.exists())

    def test_unqualified_release_even_dry_run_never_dispatches(self):
        result = self.call(self.head, environment=self.environment | {'TL3_FIXTURE_GATE': '37', 'TL3_DEPLOY_DRY_RUN': '1'})
        self.assertEqual(result.returncode, 37)
        self.assertFalse(self.marker.exists())

    def test_wrong_missing_short_head_and_dirt_refuse(self):
        for args in ([], ['a' * 40], ['main'], [self.head[:8]], [self.head, self.head]):
            self.assertNotEqual(self.call(*args).returncode, 0)
        (self.repo / 'dirty').write_text('dirt')
        self.assertNotEqual(self.call(self.head).returncode, 0)
        self.assertFalse(self.marker.exists())

    def test_native_source_identity_uses_generated_history_and_refuses_drift(self):
        self.git('remote', 'add', 'origin', fx.UPSTREAM)
        selected = self.root / self.head
        self.repo.rename(selected)
        self.repo = selected
        with patch.object(fx, 'SOURCE', self.repo), patch.object(fx, 'SOURCE_STORE', self.root):
            self.assertEqual(fx.source_identity(self.head), self.git('rev-parse', 'HEAD^{tree}').decode().strip())
            for head in ('a' * 40, 'main', ''):
                with self.assertRaises(RuntimeError):
                    fx.source_identity(head)
            (self.repo / 'ignored').write_text('dirt')
            with self.assertRaises(RuntimeError):
                fx.source_identity(self.head)


if __name__ == '__main__':
    unittest.main()
