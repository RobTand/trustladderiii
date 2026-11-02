import contextlib
import importlib.util
import io
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('deployment_mlflow', Path(__file__).with_name('deploy.py'))
deploy = importlib.util.module_from_spec(spec)
spec.loader.exec_module(deploy)


class MlflowDeployTests(unittest.TestCase):
    def test_unit_orders_postgres_and_uses_only_its_credential(self):
        unit = Path(__file__).with_name('units') / 'tl3-mlflow.service'
        self.assertTrue(unit.exists(), 'MLflow user unit is missing')
        text = unit.read_text()
        self.assertIn('Requires=tl3-postgres.service', text)
        self.assertIn('After=tl3-postgres.service', text)
        self.assertIn('LoadCredential=postgres-password:', text)
        self.assertNotIn('model-key', text)
        self.assertNotIn('tl3d.service', text)
        self.assertIn('Restart=on-failure', text)

    def test_deploy_check_reports_mlflow_health(self):
        self.assertTrue(callable(getattr(deploy, 'mlflow_health', None)), 'MLflow health check is missing')
        with tempfile.TemporaryDirectory() as directory:
            home = Path(directory)
            deploy.write_file(home / '.config/tl3/mlflow.toml',
                              b'[mlflow]\ntracking_uri="http://192.168.1.107:5033"\nexperiment="tl3"\n')
            with patch.object(deploy.http.client, 'HTTPConnection') as connection:
                connection.return_value.getresponse.return_value.status = 200
                self.assertTrue(deploy.mlflow_health(home))
                connection.return_value.request.assert_called_once_with('GET', '/health')
            with patch.object(deploy.http.client, 'HTTPConnection') as connection:
                connection.return_value.request.side_effect = OSError('fixture only')
                with contextlib.redirect_stdout(io.StringIO()) as output:
                    self.assertFalse(deploy.mlflow_health(home))
                self.assertIn('MISSING MLflow HTTP', output.getvalue())


class MlflowHostsTests(unittest.TestCase):
    def test_launcher_accepts_ip_hostname_and_mdns_for_hosts_and_cors(self):
        spec = importlib.util.spec_from_file_location('mlflow_server', Path(__file__).with_name('mlflow-server.py'))
        launcher = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(launcher)
        build = getattr(launcher, 'access_args', None)
        self.assertTrue(callable(build), 'launcher needs testable host/CORS arguments')
        args = build('http://192.168.1.107:5033', hostname='dl380g10')
        hosts = args[args.index('--allowed-hosts') + 1].split(',')
        origins = args[args.index('--cors-allowed-origins') + 1].split(',')
        for host in ['192.168.1.107', '127.0.0.1', 'localhost', 'dl380g10', 'dl380g10.local']:
            self.assertIn(host + ':5033', hosts)
            self.assertIn('http://' + host + ':5033', origins)
        self.assertNotIn('*', ','.join(args))
