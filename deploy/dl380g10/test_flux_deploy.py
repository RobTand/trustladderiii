"""Hermetic fake-native controls. These tests never invoke a real Flux client."""
import ast
import copy
import fcntl
import hashlib
import importlib.util
import json
import os
import re
import resource
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
        self.no_result = False
        self.failure_status = None
        self.failure_events = None
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
            if args[1] == 'jobid-path':
                return 0, b'/sys', b''
            value = ('different' if self.drift else '1794024934.96') if args[1] == 'broker.starttime' else '3'
            return 0, value.encode(), b''
        if args[:2] == ('config', 'get'):
            return 0, fx.encoded({'exec': {'service': 'sdexec', 'sdexec-constrain-resources': False,
                    'sdexec-properties': {'MemoryMax': '75%', 'MemorySwapMax': '0', 'Slice': 'flux-jobs.slice'}}}), b''
        if args[:2] == ('submit', '--dry-run'):
            self.assertEqual(args[2], '--env=-*')
            self.assertIn('-n1', args)
            self.assertNotIn('-N1', args)
            self.assertFalse(any(arg.startswith(('-c', '-g')) for arg in args))
            self.assertIn(f"--resources-shape=slot/[core={self.intent['cpus']};memory={self.intent['memory_gib']}]", args)
            self.assertIn('-t5400s', args)
            self.assertIn('--requires=host:dl380g10', args)
            env = dict(item[len('--env='):].split('=', 1) for item in args[3:] if item.startswith('--env='))
            self.assertEqual(env, self.intent['environment'])
            self.assertNotIn('TL3_PLANTED_MARKER', env)
            command = list(args[args.index('/usr/bin/python3'):])
            system = {'environment': env, 'cwd': str(self.source), 'duration': 5400,
                      'constraints': {'hostlist': ['dl380g10']}}
            if '--rlimit=-*' not in args:
                # Model the Flux CLI: it propagates the submitter's soft rlimits unless reset.
                system['shell'] = {'options': {'rlimit': {
                    'nofile': resource.getrlimit(resource.RLIMIT_NOFILE)[0],
                    'core': resource.getrlimit(resource.RLIMIT_CORE)[0]}}}
            return 0, fx.encoded({'version': 1, 'resources': [{'type': 'slot', 'count': 1, 'label': 'task',
                    'with': [{'type': 'core', 'count': self.intent['cpus']}, {'type': 'memory', 'count': self.intent['memory_gib']}]}],
                    'tasks': [{'command': command, 'slot': 'task', 'count': {'per_slot': 1}}],
                    'attributes': {'system': system}}), b''
        if args[:2] == ('job', 'submit'):
            self.spec = fx.load(Path(args[2]))
            expected = fx.digest(fx.encoded(self.intent))
            for name in ('stdout', 'stderr'):
                fx.save(self.out / name, b'fixture output\n')
            fx.save(self.out / 'result.json', {'schema': 'tl3.deploy.payload.v1', 'intent': expected,
                    'job_id': self.identifier, 'returncode': self.command_failure, 'source_tree': self.intent['tree'],
                    'scratch_removed': True, 'cgroup': '/fixture-absent/flux-jobs.slice/shell-fixture:sys.service',
                    'cgroup_inode': 123})
            if self.no_result:
                (self.out / 'result.json').unlink()
                if not any(row['name'] == 'start' for row in self.failure_events):
                    (self.out / 'stdout').unlink()
                    (self.out / 'stderr').unlink()
            if self.lost_reply:
                return 74, b'', b'fixture lost submit reply\n'
            return 0, (self.identifier + '\n').encode(), b''
        if args[:2] == ('jobs', '-a'):
            return 0, fx.encoded({'jobs': [{'jobid': self.identifier, 'name': self.intent['name']}
                                          for _ in range(self.matches)]}), b''
        if args[:3] == ('job', 'info', '--original'):
            if args[-2] != self.identifier:
                return 1, b'', b'unknown fixture job ID'
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
            if self.failure_status is not None:
                return self.failure_rc, fx.encoded(self.failure_status), b''
            value = {} if self.bad_status else {'id': 42, 'exception_occurred': False,
                    'waitstatus': self.command_failure << 8, 'result': 2 if self.command_failure else 1}
            return (1 if self.terminal_failure else self.command_failure), fx.encoded(value), b''
        if args[:2] == ('job', 'eventlog'):
            if self.failure_events is not None:
                return 0, b'\n'.join(fx.encoded(row) for row in self.failure_events), b''
            value = [{'name': 'start'}, {'name': 'finish', 'context': {'status': self.command_failure << 8}}, {'name': 'clean'}]
            if self.terminal_failure:
                value.insert(1, {'name': 'exception', 'context': {'type': 'exec', 'severity': 0}})
            return 0, b'\n'.join(fx.encoded(row) for row in value), b''
        if args[:2] == ('job', 'info') and args[-1] == 'R':
            return 0, fx.encoded({'version': 1, 'execution': {'R_lite': [{'rank': '0', 'children': {'core': '0-' + str(self.intent['cpus'] - 1)}}], 'nodelist': ['dl380g10']}}), b''
        if args[:2] == ('job', 'id'):
            return 0, b'42', b''
        self.fail('unexpected native fixture command')

    def test_x7_plain_memory_ceiling_is_24_gib(self):
        self.intent['memory_gib'] = 25
        with self.assertRaisesRegex(RuntimeError, 'resource'):
            spec = fx.jobspec(self.intent, fx.digest(fx.encoded(self.intent)))
            fx.validate_spec(self.intent, fx.digest(fx.encoded(self.intent)), spec)

    def test_x7_broker_restart_is_receipt_evidence(self):
        self.drift = True
        fx.binding()

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

    def test_dry_run_clears_propagated_rlimits(self):
        spec = fx.jobspec(self.intent, fx.digest(fx.encoded(self.intent)))
        dry_runs = [args for args in self.calls if args[:2] == ('submit', '--dry-run')]
        self.assertEqual(len(dry_runs), 1)
        self.assertEqual(dry_runs[0].count('--rlimit=-*'), 1, dry_runs[0])
        self.assertNotIn('rlimit', spec['attributes']['system'].get('shell', {}).get('options', {}))
        # The fake models Flux: without the reset the submitter's limits reach the jobspec.
        _, raw, _ = self.native(*[item for item in dry_runs[0] if item != '--rlimit=-*'])
        leaked = json.loads(raw)['attributes']['system']['shell']['options']['rlimit']
        self.assertEqual(leaked['nofile'], resource.getrlimit(resource.RLIMIT_NOFILE)[0])
        self.assertEqual(self.submissions(), 0)

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
        self.assertIn('instance-observation.json', receipt['files'])
        self.assertEqual(fx.load(self.out / 'binding.json')['job_id'], self.identifier)
        slot = self.spec['resources'][0]
        self.assertEqual(slot['with'], [{'type': 'core', 'count': 8}, {'type': 'memory', 'count': 8}])
        self.assertEqual(self.spec['attributes']['system']['environment']['CARGO_BUILD_JOBS'], '2')
        self.assertEqual(self.spec['attributes']['user']['source_head'], 'a' * 40)
        self.assertEqual(self.spec['attributes']['system']['duration'], 5400)
        self.assertTrue(any('--requires=host:dl380g10' in call for call in self.calls))
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

    def test_changed_generation_is_observed_before_admission(self):
        self.drift = True
        self.assertEqual(fx.binding()['broker_starttime'], 'different')
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

    def prepare_no_result(self, started):
        self.failure_case = getattr(self, 'failure_case', 0) + 1
        self.out = self.root / f'failure-{self.failure_case}' / 'build'
        self.out.parent.mkdir()
        self.intent = self.intent | {'out': str(self.out), 'name': f'tl3-deploy-failure-{self.failure_case}-build'}
        patch.object(fx.socket, 'gethostname', return_value='dl380g10').start()
        self.no_result = True
        self.failure_rc = 143 if started else 125
        self.failure_status = {'id': 42, 'exception_occurred': True,
                               'exception_type': 'cancel', 'exception_severity': 0, 'result': 4}
        self.failure_events = [{'name': 'submit', 'context': {'userid': os.getuid()}}]
        if started:
            self.failure_events += [{'name': 'alloc'}, {'name': 'start'}]
            self.failure_status['waitstatus'] = 15
        self.failure_events += [{'name': 'exception', 'context': {'type': 'cancel', 'severity': 0}}]
        if started:
            self.failure_events += [{'name': 'finish', 'context': {'status': 15}}]
        self.failure_events += [{'name': 'clean'}]
        for number, row in enumerate(self.failure_events):
            row['timestamp'] = float(number + 1)
        uid = os.getuid()
        self.failure_slice = fx.CGROUP_ROOT / f'user.slice/user-{uid}.slice/user@{uid}.service/flux-jobs.slice'
        self.failure_slice.mkdir(parents=True, exist_ok=True)

    def assert_failed_closed(self):
        with self.assertRaises((RuntimeError, OSError)):
            fx.stage(self.intent)
        self.assertTrue((self.out / 'closed.json').exists(), 'proven ended job wedges admission')
        closed = fx.load(self.out / 'closed.json')
        self.assertEqual(closed['schema'], 'tl3.deploy.closed.v2')
        self.assertEqual(closed['outcome'], 'FAILED')
        self.assertFalse((self.out / 'receipt.json').exists())
        self.assertFalse((self.out.parent / 'accepted.json').exists())
        self.assertFalse((self.out / 'result.json').exists())
        with patch.object(fx, 'admit_stages', return_value={}) as admitted:
            self.coordinator(self.root)
            admitted.assert_called_once()
        self.assertEqual(self.submissions(), 1)

    def assert_failure_held(self):
        submitted = self.submissions()
        with self.assertRaises((RuntimeError, ValueError, KeyError, TypeError, OSError)):
            fx.stage(self.intent)
        self.assertFalse((self.out / 'closed.json').exists())
        with patch.object(fx, 'admit_stages', return_value={}) as admitted:
            with self.assertRaises(RuntimeError):
                self.coordinator(self.root)
            admitted.assert_not_called()
        self.assertEqual(self.submissions(), submitted + 1)

    def test_cancelled_before_start_without_result_allows_failed_closure(self):
        self.prepare_no_result(False)
        self.assert_failed_closed()

    def test_sigterm_mid_run_without_result_allows_failed_closure(self):
        self.prepare_no_result(True)
        self.assert_failed_closed()

    def test_no_result_contradictory_terminal_records_hold_admission(self):
        cases = ('wrong-id', 'missing-id', 'success-result', 'wrong-result', 'missing-exception',
                 'wrong-exception-type', 'nonfatal-exception', 'wrong-rc', 'wrong-waitstatus',
                 'missing-clean', 'running', 'duplicate-start', 'missing-finish', 'missing-alloc',
                 'reordered', 'bad-timestamp', 'wrong-uid', 'malformed-status', 'stopped')
        for case in cases:
            with self.subTest(case=case):
                self.prepare_no_result(True)
                if case == 'wrong-id':
                    self.failure_status['id'] = 43
                elif case == 'missing-id':
                    del self.failure_status['id']
                elif case == 'success-result':
                    self.failure_status['result'] = 1
                elif case == 'wrong-result':
                    self.failure_status['result'] = 8
                elif case == 'missing-exception':
                    self.failure_status['exception_occurred'] = False
                elif case == 'wrong-exception-type':
                    self.failure_status['exception_type'] = 'timeout'
                elif case == 'nonfatal-exception':
                    self.failure_events[3]['context']['severity'] = 1
                elif case == 'wrong-rc':
                    self.failure_rc = 0
                elif case == 'wrong-waitstatus':
                    self.failure_status['waitstatus'] = 9
                elif case in ('missing-clean', 'running', 'missing-finish', 'missing-alloc'):
                    name = {'missing-clean': 'clean', 'running': 'clean',
                            'missing-finish': 'finish', 'missing-alloc': 'alloc'}[case]
                    self.failure_events = [row for row in self.failure_events if row['name'] != name]
                elif case == 'duplicate-start':
                    self.failure_events.insert(2, dict(self.failure_events[2]))
                elif case == 'reordered':
                    self.failure_events[2], self.failure_events[4] = self.failure_events[4], self.failure_events[2]
                elif case == 'bad-timestamp':
                    self.failure_events[2]['timestamp'] = float('nan')
                elif case == 'wrong-uid':
                    self.failure_events[0]['context']['userid'] = os.getuid() + 1
                elif case == 'malformed-status':
                    self.failure_status = []
                elif case == 'stopped':
                    self.failure_status['waitstatus'] = (19 << 8) | 127
                    self.failure_events[4]['context']['status'] = self.failure_status['waitstatus']
                self.assert_failure_held()

    def test_prestart_allocation_or_finish_is_unknown(self):
        for name in ('alloc', 'finish'):
            with self.subTest(name=name):
                self.prepare_no_result(False)
                self.failure_events.insert(1, {'name': name, 'timestamp': 1.5, 'context': {'status': 15}})
                self.assert_failure_held()

    def test_no_result_cleanup_requires_absent_exact_scopes_and_scratch(self):
        for name in ('sys', '0-sys', '1-sys', '2-sys', 'scratch-unicode', 'scratch-plain', 'symlink'):
            with self.subTest(name=name):
                self.prepare_no_result(True)
                if name.startswith('scratch'):
                    target = fx.SCRATCH / (self.identifier if name == 'scratch-unicode' else 'f' + self.identifier[1:])
                    target.mkdir()
                else:
                    target = self.failure_slice / f'shell-f{self.identifier[1:]}:{name if name != "symlink" else "sys"}.service'
                    if name == 'symlink':
                        target.symlink_to(self.root / 'absent')
                    else:
                        target.mkdir()
                        (target / 'cgroup.events').write_text('populated 0\n')
                self.assert_failure_held()
                if target.is_symlink():
                    target.unlink()
                else:
                    for child in target.iterdir():
                        child.unlink()
                    target.rmdir()

    def test_no_result_unknown_slice_owner_or_identity_holds(self):
        self.prepare_no_result(True)
        original = Path.lstat
        def wrong_owner(path, *args, **kwargs):
            info = original(path, *args, **kwargs)
            if path == self.failure_slice:
                return SimpleNamespace(st_mode=info.st_mode, st_uid=os.getuid() + 1,
                                       st_dev=info.st_dev, st_ino=info.st_ino)
            return info
        with patch.object(Path, 'lstat', wrong_owner):
            self.assert_failure_held()

    def test_no_result_missing_slice_holds(self):
        self.prepare_no_result(False)
        self.failure_slice.rmdir()
        self.assert_failure_held()

    def test_failed_closure_mutation_or_new_result_blocks_next_admission(self):
        self.prepare_no_result(True)
        self.assert_failed_closed()
        for name in ('eventlog.jsonl', 'failure-cleanup.json', 'binding.json', 'jobspec.json',
                     'intent.json', 'stdout', 'failure-identity.json'):
            with self.subTest(name=name):
                path = self.out / name
                original = path.read_bytes()
                fx.save(path, original + b' ')
                with patch.object(fx, 'admit_stages') as admitted:
                    with self.assertRaises(RuntimeError):
                        self.coordinator(self.root)
                    admitted.assert_not_called()
                fx.save(path, original)
        fx.save(self.out / 'result.json', {})
        with patch.object(fx, 'admit_stages') as admitted:
            with self.assertRaises(RuntimeError):
                self.coordinator(self.root)
            admitted.assert_not_called()
        self.assertEqual(self.submissions(), 1)

    def test_failed_closure_cannot_exempt_an_unresolved_sibling(self):
        self.prepare_no_result(False)
        self.assert_failed_closed()
        other = self.root / 'unresolved' / 'build'
        other.mkdir(parents=True)
        fx.save(other / 'dispatched.json', {})
        with patch.object(fx, 'admit_stages') as admitted:
            with self.assertRaises(RuntimeError):
                self.coordinator(self.root, '--reattach', str(self.out.parent))
            admitted.assert_not_called()
        self.assertEqual(self.submissions(), 1)

    def test_new_attempt_after_failed_closure_gets_new_job_id(self):
        self.prepare_no_result(True)
        self.assert_failed_closed()
        previous = self.out
        self.no_result = False
        self.failure_status = None
        self.failure_events = None
        self.identifier = 'ƒ456def'
        self.out = self.root / 'new-attempt' / 'build'
        self.out.parent.mkdir()
        self.intent = self.intent | {'out': str(self.out), 'name': 'tl3-deploy-new-attempt-build'}
        with patch.object(fx, 'STORE', self.root):
            with fx.serialized_admission(None):
                fx.stage(self.intent)
        self.assertEqual(self.submissions(), 2)
        self.assertNotEqual(fx.load(previous / 'binding.json')['job_id'], fx.load(self.out / 'binding.json')['job_id'])
        self.assertFalse((previous / 'receipt.json').exists())
        self.assertTrue((self.out / 'receipt.json').exists())

    def test_failed_closure_wrong_binding_or_outcome_refuses_without_submit(self):
        self.prepare_no_result(False)
        self.assert_failed_closed()
        for name, key, value in (('binding.json', 'job_id', 'ƒ456def'),
                                  ('binding.json', 'intent', '0' * 64),
                                  ('binding.json', 'jobspec_sha256', '0' * 64),
                                  ('intent.json', 'generation', 'other-generation'),
                                  ('closed.json', 'outcome', 'PASS')):
            with self.subTest(name=name, key=key):
                path = self.out / name
                original = path.read_bytes()
                altered = fx.load(path)
                altered[key] = value
                fx.save(path, altered)
                with patch.object(fx, 'admit_stages') as admitted:
                    with self.assertRaises(RuntimeError):
                        self.coordinator(self.root)
                    admitted.assert_not_called()
                if name != 'closed.json':
                    with self.assertRaises(RuntimeError):
                        fx.stage(self.intent)
                fx.save(path, original)
        self.assertEqual(self.submissions(), 1)
        self.assertFalse((self.out / 'receipt.json').exists())

    def test_failed_closure_evidence_absence_is_bound(self):
        self.prepare_no_result(False)
        self.assert_failed_closed()
        for name in ('stdout', 'stderr', 'receipt.json'):
            with self.subTest(name=name):
                fx.save(self.out / name, b'new file')
                with patch.object(fx, 'admit_stages') as admitted:
                    with self.assertRaises(RuntimeError):
                        self.coordinator(self.root)
                    admitted.assert_not_called()
                (self.out / name).unlink()
        self.assertEqual(self.submissions(), 1)

    def test_failed_closure_host_generation_and_query_unknown_hold(self):
        for case in ('wrong-host', 'wrong-instance', 'id-query-failed', 'events-query-failed'):
            with self.subTest(case=case):
                self.prepare_no_result(True)
                original = self.native
                def native(*args, **kwargs):
                    if case == 'wrong-instance' and args == ('getattr', 'jobid-path'):
                        return 0, b'/other', b''
                    if case == 'id-query-failed' and args[:2] == ('job', 'id'):
                        return 74, b'42', b''
                    if case == 'events-query-failed' and args[:2] == ('job', 'eventlog'):
                        _, value, error = original(*args, **kwargs)
                        return 74, value, error
                    return original(*args, **kwargs)
                if case == 'wrong-generation':
                    self.drift = True
                with patch.object(fx, 'native', native), patch.object(
                        fx.socket, 'gethostname', return_value='other' if case == 'wrong-host' else 'dl380g10'):
                    self.assert_failure_held()
                self.drift = False

    def test_started_exit_failure_without_exception_can_only_close_failed(self):
        self.prepare_no_result(True)
        self.failure_events = [row for row in self.failure_events if row['name'] != 'exception']
        self.failure_events[-2]['context']['status'] = 7 << 8
        self.failure_status = {'id': 42, 'exception_occurred': False, 'waitstatus': 7 << 8, 'result': 2}
        self.failure_rc = 7
        self.assert_failed_closed()

    def test_failed_closure_recreated_cleanup_or_changed_root_blocks_next_admission(self):
        self.prepare_no_result(True)
        self.assert_failed_closed()
        for target in (self.failure_slice / f'shell-f{self.identifier[1:]}:sys.service',
                       fx.SCRATCH / self.identifier):
            with self.subTest(path=target):
                target.mkdir()
                with patch.object(fx, 'admit_stages') as admitted:
                    with self.assertRaises(RuntimeError):
                        self.coordinator(self.root)
                    admitted.assert_not_called()
                target.rmdir()
        original = Path.lstat
        def changed_inode(path, *args, **kwargs):
            info = original(path, *args, **kwargs)
            if path == self.failure_slice:
                return SimpleNamespace(st_mode=info.st_mode, st_uid=info.st_uid,
                                       st_dev=info.st_dev, st_ino=info.st_ino + 1)
            return info
        with patch.object(Path, 'lstat', changed_inode), patch.object(fx, 'admit_stages') as admitted:
            with self.assertRaises(RuntimeError):
                self.coordinator(self.root)
            admitted.assert_not_called()
        self.assertEqual(self.submissions(), 1)

    def test_occupied_or_foreign_scope_without_result_never_closes(self):
        self.prepare_no_result(True)
        scope = self.failure_slice / f'shell-f{self.identifier[1:]}:sys.service'
        scope.mkdir()
        (scope / 'cgroup.events').write_text('populated 1\n')
        original = Path.lstat
        def foreign(path, *args, **kwargs):
            info = original(path, *args, **kwargs)
            if path == scope:
                return SimpleNamespace(st_mode=info.st_mode, st_uid=os.getuid() + 1,
                                       st_dev=info.st_dev, st_ino=info.st_ino)
            return info
        with patch.object(Path, 'lstat', foreign):
            self.assert_failure_held()

    def test_reattach_failed_job_collects_original_id_without_resubmission(self):
        self.prepare_no_result(True)
        self.assert_failed_closed()
        with self.assertRaises(RuntimeError):
            fx.stage(self.intent)
        self.assertEqual(self.submissions(), 1)
        self.assertEqual(fx.load(self.out / 'closed.json')['outcome'], 'FAILED')
        self.assertFalse((self.out / 'receipt.json').exists())

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
        self.assertNotIn('FLUX_URI', popen.call_args.kwargs['env'])

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
