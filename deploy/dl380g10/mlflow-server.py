#!/usr/bin/env python3
"""Systemd-only entry point. Read the database credential only at runtime."""
import os
from pathlib import Path
import socket
import sys
from urllib.parse import urlsplit
import tomllib


def access_args(tracking_uri, *, hostname=None):
    address = urlsplit(tracking_uri)
    hostname = hostname or socket.gethostname()
    short = hostname.split('.')[0]
    hosts = list(dict.fromkeys([address.hostname, '127.0.0.1', 'localhost',
                               hostname, short, short + '.local']))
    authorities = [f'{host}:{address.port}' for host in hosts]
    return ['--allowed-hosts', ','.join(authorities),
            '--cors-allowed-origins', ','.join(f'{address.scheme}://{host}' for host in authorities)]


def main():
    from sqlalchemy import URL
    import psycopg

    root = Path.home() / '.local/share/tl3'
    config = tomllib.loads((Path.home() / '.config/tl3/mlflow.toml').read_text())['mlflow']
    address = urlsplit(config['tracking_uri'])
    password = (Path(os.environ['CREDENTIALS_DIRECTORY']) / 'postgres-password').read_text().strip()
    connection = dict(host=str(root / 'postgres/socket'), port=5543, user='tl3', password=password,
                      connect_timeout=5)
    with psycopg.connect(**connection, dbname='postgres', autocommit=True) as database:
        if not database.execute("SELECT 1 FROM pg_database WHERE datname = 'tl3_mlflow'").fetchone():
            database.execute('CREATE DATABASE tl3_mlflow OWNER tl3')
    # Pass the URI through the server environment, never argv or a config file.
    os.environ['MLFLOW_BACKEND_STORE_URI'] = URL.create(
        'postgresql+psycopg', username='tl3', password=password, database='tl3_mlflow',
        query={'host': str(root / 'postgres/socket'), 'port': '5543'},
    ).render_as_string(hide_password=False)
    artifacts = root / 'mlflow/artifacts'
    artifacts.mkdir(parents=True, exist_ok=True, mode=0o700)
    os.chdir(root / 'mlflow')
    from mlflow.cli import server
    server.main(args=['--host', address.hostname, '--port', str(address.port), '--workers', '1',
                      '--artifacts-destination', str(artifacts),
                      *access_args(config['tracking_uri'])],
                standalone_mode=False)


if __name__ == '__main__':
    try:
        main()
    except Exception:
        print('tl3 MLflow startup failed; details suppressed', file=sys.stderr)
        raise SystemExit(1)
