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
