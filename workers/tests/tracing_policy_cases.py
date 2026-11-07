"""Input-only producer cases retained for Rust redaction qualification. No redactor oracle."""
import json
from pathlib import Path
import sys
sys.path.insert(0, str(Path(__file__).parent))
import tracing_forms
import tracing_forms3
from tracing_forms import FORMS, PLANTED, PLANTED_LINES

DOCUMENTED_LIMITS = {'DOC_unquoted_spaces', 'url_no_scheme', 'generic_webhook_path', 'cli_gh_with_token'}


ORDINARY = [
    'def test_password_reset_sends_email(client):',
    'The password must be at least 12 characters and contain a digit.',
    'Users reset their password by clicking the link in the email.',
    'for token in tokens:\n    count += 1',
    'self.token_count += len(tokens)',
    'max_tokens=4096',
    'sorted(items, key=lambda kv: kv[1])',
    'max(words, key=len)',
    'if token == expected:',
    'def verify(token: str, key: bytes) -> bool:',
    '# TODO: rotate the API key every 90 days',
    'class TokenBucket:\n    def __init__(self, capacity, rate):',
    "tokenizer = AutoTokenizer.from_pretrained('gpt2')",
    "assert 'password' in error_message",
    "git commit -m 'Fix password hashing'",
    "key = d.get('name')",
    'input_tokens: 123, output_tokens: 456',
    'The cache key is derived from the file path.',
    'export PATH=/usr/bin:$PATH',
    'HTTP 401: invalid token',
    'ssh -p 2222 host',
    'pytest -p no:cacheprovider -q workers/tests',
    "python -m pytest -k 'test_token or test_key' -q",
    'primary_key=True, sort_keys=True',
    'https://github.com/RobTand/trustladderiii/pull/302',
    'git@github.com:RobTand/trustladderiii.git',
    'ssh://git@github.com/o/r.git',
    'http://localhost:8080/path@2x.png',
    'email bob@example.com: thanks',
    'let token = self.next_token()?;',
    'for (key, value) in map.iter() {',
    'fn parse_token(s: &str) -> Option<Token> {',
    'The authorization header is required for this endpoint.',
    'Missing password for user bob; ask the requester.',
    '- [ ] rotate secrets quarterly',
    'secret handshake protocol (SHP) v2 is documented in docs/shp.md',
    'a passkey is not a password',
    'use `--token-budget 4000` to cap spend',
    'Cookie banner copy lives in web/consent.html',
    'commit 29280f4b147fd889f848c51300ba3352b9e26d9d and 3f14146feb2902d35819403c0a83bb2a0ead9a36',
    'sha256:81a462f2634e11c5372eb8822209d17cf6ca5ad653cda0315171c3a26b396cfe',
    'uuid 6f59aec1-88d6-170c-0ce7-0fdb4eebbb70',
    'Bearer tokens are described in RFC 6750.',
    'Basic usage: run `tl3 start`.',
    'basic authentication is disabled',
    'a key-value store',
    'keyboard shortcuts: ctrl+s to save',
    'monkey patch the key function',
    'tokens per second: 41.7',
    'trace tr-6f59aec188d6170c0ce70ecdb4fde685, request cdc57fa99768407bb915874b3d9fde54',
    'integrity sha512-Q2xvdWRmbGFyZS1zZWN1cml0eS10ZXN0LXZhbHVlLWZvci1pbnRlZ3JpdHk=',
]


SENSITIVE_PATHS = ['.env', 'config/.env.local', 'deploy/server.pem', 'tls/site.key', '/home/u/.ssh/id_ed25519',
                   '.netrc', 'web/.npmrc', '.pypirc', 'aws/credentials', 'k8s/db-secret.yaml',
                   '/home/u/.docker/config.json', 'keys/putty.ppk', 'certs/client.p12', '/home/u/.kube/config',
                   # Third re-check: match path components, not just base names.
                   'config/prod.env', 'deploy/app.env', 'deploy/new.env', 'env.production', 'env', 'id_rsa.bak',
                   'id_rsa.old', 'id_ed25519.orig', 'keystore.jks', 'app.keystore', 'signing.p8', 'key.pk8',
                   'secrets/prod.yml', 'ops/credentials/db.txt', '.dockercfg', 'terraform.tfvars',
                   'prod.auto.tfvars', 'terraform.tfstate', 'terraform.tfstate.backup', '.htpasswd', '.my.cnf',
                   'service-account.json', 'service-account-prod.json', ' .env']


ADVERSARIAL = ['key', 'token', 'token=', 'secret_', 'password', 'credential', 'signature', 'a.', 'a-', 'a:',
               'a://', 'sk-', 'Bearer ', 'authorization:', 'Authorization: Basic ', 'cookie=', '"password": ',
               '\\"', '\\\\\\"', "'", '--password ', '-u a:', '-u ', 'https://u:', 'eyJ.', 'ghp_', 'AKIA',
               '-----BEGIN PRIVATE KEY-----\n', '-----BEGIN A A A ', ' ', '\t', '\\n', 'token\\n', 'key=\\n ',
               '= ', 'x_token_', 'secret secret ', 'key=key=', 'password:  ', 'password="', '| ', '[a]\n',
               'name: a\n', 'x: |\n', 'mysql ', 'Private-Lines: 1\n', 'aB3', '//', ':@',
               # Third re-check: a run of backslashes was quadratic.
               '\\', '\\\\', '\\\\\\', 'password is ', 'token ', 'IDENTIFIED BY ', '<password>',
               'a,password,', '"name": "DB_PASSWORD", ', '\tTRUE\t', 'net use ', '-db-password ']


V3_DOCUMENTED = {
    'kubectl_get_secret_jsonpath_decoded',  # a bare value on the line after a command
    'env_uncommon_hex64', 'env_hmac_seed_hex64', 'env_openssl_rand_hex_output', 'hex_in_json_secret_bytes',  # 64-hex
    'unicode_escaped_json',  # a label spelled with JSON escapes inside other text
}


V3_FORMS = [(name, text, secrets) for name, text, secrets, _ in tracing_forms3.FORMS if name not in V3_DOCUMENTED]


def _variants():
    r, pw = tracing_forms3.r, lambda: tracing_forms3.r(20, tracing_forms3.PWCH)
    alnum, hex_ = tracing_forms3.ALNUM, tracing_forms3.HEX
    found = []

    def v(name, template, secret):
        found.append((name, template % secret, secret))
    v('kubectl_pod_json_env_pretty', '$ kubectl get pod web -o json\n{\n  "spec": {"containers": [{"env": [\n    {\n'
      '      "name": "DB_PASSWORD",\n      "value": "%s"\n    }\n  ]}]}\n}', pw())
    v('json_key_value_inline', 'vars: {"key":"DB_PASSWORD","value":"%s"} end', pw())
    v('json_key_value_escaped', 'out: {\\"name\\": \\"API_TOKEN\\", \\"value\\": \\"%s\\"}', pw())
    v('form_body_pct_leading', 'username=bob&password=%s&client=cli', '%21' + r(16, alnum))
    v('sql_with_password', "CREATE USER orders_svc WITH PASSWORD '%s';", pw())
    v('sql_alter_user', "ALTER USER app WITH PASSWORD '%s';", pw())
    v('sql_create_role_login', "CREATE ROLE svc LOGIN PASSWORD '%s' NOSUPERUSER;", pw())
    v('sql_mysql_set_password', "SET PASSWORD FOR 'app'@'%%' = PASSWORD('%s');", pw())
    v('sql_identified_by', "CREATE USER 'app'@'%%' IDENTIFIED BY '%s';", pw())
    v('xml_element', '<server><id>nexus</id><username>deploy</username><password>%s</password></server>', pw())
    v('xml_maven_settings', '<settings><servers><server>\n  <id>central</id>\n  <username>deploy</username>\n'
      '  <password>%s</password>\n</server></servers></settings>', pw())
    v('xml_hibernate_property', '<property name="hibernate.connection.password">%s</property>', pw())
    v('csv_password_column', 'username,password,role\nbob,%s,admin', pw())
    v('csv_row_with_header_password', 'id,email,password_hash,password\n1,a@b.io,x,%s', pw())
    v('prose_password_is', 'The password for the staging database is %s - please rotate it.', pw())
    v('prose_pw_is', 'the db pw is %s', pw())
    v('prose_use_password', 'You can log in with user admin and password %s.', pw())
    v('prose_password_used', 'FATAL: password authentication failed for user "app" (password used: %s)', pw())
    v('prose_login_pair', 'Login: admin / %s', pw())
    v('redis_requirepass', 'requirepass %s', pw())
    v('redis_auth_command', 'AUTH %s', pw())
    v('go_flag_password', 'myapp -db-password %s -port 8080', pw())
    v('single_dash_flag', 'tool -pass %s', pw())
    v('powershell_flag', 'Connect-Server -Password "%s"', pw())
    v('powershell_securestring', "$p = ConvertTo-SecureString '%s' -AsPlainText -Force", pw())
    v('net_use', 'net use \\\\srv\\share /user:CORP\\bob %s', pw())
    v('htpasswd_apr1', 'deploy:$apr1$h7Gk2dQ1$%s', r(22, alnum))
    v('netscape_cookie_jar', '.acme.io\tTRUE\t/\tTRUE\t1893456000\tsession_id\t%s', r(40, hex_))
    v('airtable_pat', 'token %s', 'pat' + r(14) + '.' + r(64, hex_))
    return found


VARIANTS = _variants()


ORDINARY_V3 = [
    "reply={'ok': True, 'scope_id': unit, 'token': 'b'*64,",
    'token = "w" * 32',
    'env_path.write_text("GF_SECURITY_ADMIN_PASSWORD=" + password + "\\nGF_USERS_DEFAULT_THEME=dark")',
    'ssh-keygen -q -t ed25519 -N "" -C "pb-smoke3-$stamp" -f "$ctx/id_smoke" || exit 2',
    '"Authorization": self.authorization, "Content-Type": "application/json"})',
    'scope `prismabuild-jobd3c6002f82352acc91a2998304cbe0e2.slice` on Sparklina.',
    "'uuid': 'GPU-b1eceeea-fec7-371e-2cf3-cd10f2e7b705',",
    'receipt      8ff38b7dc24d51c63e4147dbe1587cf1...   cas_receipt.v3',
    '"10.batch|00:00:00|00:00:00|20164608|5600628|33215|1|"',
    'if not password:\n    continue',
    'Called after release with the same attempt token: reports',
    '"unit": "pb-test-inventory-9cb03d044a0faeabe29b.service",',
    'Rotate the token every 90 days from the admin console.',
    'password = getpass("Password: ")',
    'The token expires after 3600 seconds unless refreshed.',
    'Secret rotation runs nightly at 02:00 UTC for every service.',
    'Store the password in the vault, not in the repository.',
    'let token = tokens.next().ok_or(Error::MissingToken)?;',
    'username,password_hash,role',
]



def main():
    import sys
    from pathlib import Path
    manifest = json.loads(Path(sys.argv[1]).read_text())
    assert manifest == {'marker': 'TL3FAKE', 'seeds': [tracing_forms.SEED, tracing_forms3.SEED]}
    forms = [(name, text, secrets) for name, text, secrets, _ in FORMS if name not in DOCUMENTED_LIMITS]
    forms += V3_FORMS + [(name, text, [secret]) for name, text, secret in VARIANTS]
    forms += [(f'planted-{index}', text, [secret]) for index, (text, secret) in enumerate(PLANTED + PLANTED_LINES)]
    result = {'forms': forms, 'ordinary': ORDINARY + ORDINARY_V3,
              'sensitive_paths': SENSITIVE_PATHS, 'adversarial': ADVERSARIAL}
    print(json.dumps(result))


if __name__ == '__main__':
    main()
