"""Planted credential forms from the #302 review. Every secret is a random fake from a fixed seed."""
import base64
import json
import random
import string

rng = random.Random(20261101)
ALNUM = string.ascii_letters + string.digits
HEX = '0123456789abcdef'
B64 = string.ascii_letters + string.digits + '+/'
B64U = string.ascii_letters + string.digits + '-_'
UPPER = string.ascii_uppercase + string.digits


def r(n, alpha=ALNUM):
    return ''.join(rng.choice(alpha) for _ in range(n))


def b64(s):
    return base64.b64encode(s.encode()).decode()


def jwt():
    seg = lambda n: base64.urlsafe_b64encode(r(n).encode()).decode().rstrip('=')
    return 'eyJ' + seg(30) + '.' + 'eyJ' + seg(60) + '.' + seg(43)


def pem(kind, lines=6):
    body = '\n'.join(r(64, B64) for _ in range(lines))
    return f'-----BEGIN {kind}-----\n{body}\n-----END {kind}-----', body.split('\n')


FORMS = []   # (name, text, [secrets], doc_tag)


def add(name, template, n_secrets=1, gen=None, doc=None):
    secrets = [(gen() if gen else r(24)) for _ in range(n_secrets)]
    FORMS.append((name, template.format(*secrets), secrets, doc))


def raw(name, text, secrets, doc=None):
    FORMS.append((name, text, secrets, doc))


# ---- headers ----
add('hdr_bearer', 'Authorization: Bearer {0}', gen=lambda: r(40))
add('hdr_bearer_lower', 'authorization: bearer {0}', gen=lambda: r(40))
user_pw = b64('bob:' + r(14))
raw('hdr_basic', f'Authorization: Basic {user_pw}', [user_pw])
add('hdr_token_noprefix', 'Authorization: token {0}', gen=lambda: r(40))
add('hdr_Token_scheme', 'Authorization: Token {0}', gen=lambda: r(40))
add('hdr_apikey_scheme', 'Authorization: ApiKey {0}', gen=lambda: r(40))
sig, akid = r(64, HEX), 'AKIA' + r(16, UPPER)
raw('hdr_aws4', f'Authorization: AWS4-HMAC-SHA256 Credential={akid}/20261101/us-east-1/s3/aws4_request, '
                f'SignedHeaders=host;x-amz-date, Signature={sig}', [sig, akid[4:]])
add('hdr_digest', 'Authorization: Digest username="bob", realm="r", nonce="{0}", uri="/x", response="{1}"',
    2, gen=lambda: r(32, HEX))
add('hdr_negotiate', 'Authorization: Negotiate {0}', gen=lambda: r(60, B64))
pb = b64('proxy:' + r(12))
raw('hdr_proxy_basic', f'Proxy-Authorization: Basic {pb}', [pb])
add('hdr_proxy_noscheme', 'Proxy-Authorization: {0}', gen=lambda: r(30))
add('hdr_cookie', 'Cookie: sessionid={0}; csrftoken={1}', 2, gen=lambda: r(32))
add('hdr_set_cookie', 'Set-Cookie: sid={0}; Path=/; HttpOnly; Secure', gen=lambda: r(32))
add('hdr_x_api_key', 'X-Api-Key: {0}', gen=lambda: r(32))
add('hdr_x_api_key_lower', 'x-api-key: {0}', gen=lambda: r(32))
add('hdr_x_auth_token', 'X-Auth-Token: {0}', gen=lambda: r(32))
add('hdr_x_amz_security', 'X-Amz-Security-Token: {0}', gen=lambda: r(120, B64))
add('hdr_x_goog_api_key', 'x-goog-api-key: {0}', gen=lambda: r(32))
add('hdr_api_key_hyphen', 'Api-Key: {0}', gen=lambda: r(32))
add('hdr_private_token', 'PRIVATE-TOKEN: {0}', gen=lambda: r(32))
add('hdr_ocp_apim', 'Ocp-Apim-Subscription-Key: {0}', gen=lambda: r(32, HEX))
add('hdr_curl_H_dq', 'curl -H "Authorization: Bearer {0}" https://api.example.com/v1/x', gen=lambda: r(40))
add('hdr_curl_H_sq', "curl -H 'X-Api-Key: {0}' https://api.example.com/v1/x", gen=lambda: r(40))
add('hdr_json_headers', '{{"headers": {{"Authorization": "Bearer {0}"}}}}', gen=lambda: r(40))
add('hdr_py_repr', "{{'Authorization': 'Bearer {0}'}}", gen=lambda: r(40))
add('hdr_requests_token', "requests.get(url, headers={{'Authorization': 'token {0}'}})", gen=lambda: r(40))
add('hdr_httpie', "http GET example.com Authorization:'Bearer {0}'", gen=lambda: r(40))
add('hdr_wire_crlf', 'GET /x HTTP/1.1\r\nHost: a\r\nAuthorization: Bearer {0}\r\nAccept: */*\r\n\r\n', gen=lambda: r(40))
add('hdr_cookie_wire', 'GET / HTTP/1.1\r\nCookie: a=1; token={0}; b=2\r\nHost: x', gen=lambda: r(40))

# ---- vendor tokens ----
for p in ('ghp', 'gho', 'ghu', 'ghs', 'ghr'):
    add(f'gh_{p}', p + '_{0}', gen=lambda: r(36))
add('gh_fine_grained', 'github_pat_{0}_{1}', 2, gen=lambda: r(40, ALNUM + '_'))
add('gitlab_glpat', 'glpat-{0}', gen=lambda: r(20, B64U))
add('gitlab_glrt', 'glrt-{0}', gen=lambda: r(20, B64U))
add('gitlab_glptt', 'glptt-{0}', gen=lambda: r(40, HEX))
add('gitlab_gldt', 'gldt-{0}', gen=lambda: r(20, B64U))
add('slack_xoxb', 'xoxb-{0}-{1}-{2}', 3, gen=lambda: r(24))
add('slack_xoxp', 'xoxp-1234567890-1234567890123-1234567890123-{0}', gen=lambda: r(32, HEX))
add('slack_xoxe', 'xoxe.xoxp-1-{0}', gen=lambda: r(120))
add('slack_xapp', 'xapp-1-A0123456789-1234567890123-{0}', gen=lambda: r(64, HEX))
add('slack_xoxa', 'xoxa-2-{0}', gen=lambda: r(40))
add('stripe_sk_live', 'sk_live_{0}', gen=lambda: r(24))
add('stripe_rk_live', 'rk_live_{0}', gen=lambda: r(24))
add('stripe_sk_test', 'sk_test_{0}', gen=lambda: r(24))
add('stripe_whsec', 'whsec_{0}', gen=lambda: r(32))
add('google_AIza', 'AIza{0}', gen=lambda: r(35, B64U))
add('google_ya29', 'ya29.a0Af{0}', gen=lambda: r(90, B64U))
add('google_GOCSPX', 'GOCSPX-{0}', gen=lambda: r(28, B64U))
add('google_refresh_1//', '1//0g{0}', gen=lambda: r(60, B64U))
add('aws_AKIA', 'AKIA{0}', gen=lambda: r(16, UPPER))
add('aws_ASIA', 'ASIA{0}', gen=lambda: r(16, UPPER))
add('aws_secret_eq', 'aws_secret_access_key = {0}', gen=lambda: r(40, B64))
add('aws_secret_env', 'AWS_SECRET_ACCESS_KEY={0}', gen=lambda: r(40, B64))
add('aws_secret_json', '{{"SecretAccessKey": "{0}", "AccessKeyId": "x"}}', gen=lambda: r(40, B64))
add('aws_secret_space_sep', 'aws configure set aws_secret_access_key {0}', gen=lambda: r(40, B64))
add('aws_session_token_env', 'AWS_SESSION_TOKEN={0}', gen=lambda: r(300, B64))
add('aws_secret_bare', 'the key was {0} for the bucket', gen=lambda: r(40, B64), doc='D3-unlabeled')
add('npm_token', 'npm_{0}', gen=lambda: r(36))
add('npmrc_authToken', '//registry.npmjs.org/:_authToken={0}', gen=lambda: r(36, HEX))
add('npmrc_auth_b64', '_auth={0}', gen=lambda: b64('bob:' + r(12)))
add('pypi_token', 'pypi-AgEIcHlwaS5vcmc{0}', gen=lambda: r(100, B64U))
add('hf_token', 'hf_{0}', gen=lambda: r(34, string.ascii_letters))
add('sendgrid', 'SG.{0}.{1}', 2, gen=lambda: r(22, B64U))
add('openai_sk', 'sk-{0}', gen=lambda: r(48))
add('openai_sk_proj', 'sk-proj-{0}', gen=lambda: r(100, B64U))
add('anthropic_sk_ant', 'sk-ant-api03-{0}-AA', gen=lambda: r(93, B64U))
add('anthropic_oat', 'sk-ant-oat01-{0}', gen=lambda: r(80, B64U))
add('twilio_sk', 'SK{0}', gen=lambda: r(32, HEX))
add('twilio_auth_env', 'TWILIO_AUTH_TOKEN={0}', gen=lambda: r(32, HEX))
add('mailgun_key', 'key-{0}', gen=lambda: r(32, HEX))
add('digitalocean', 'dop_v1_{0}', gen=lambda: r(64, HEX))
add('shopify', 'shpat_{0}', gen=lambda: r(32, HEX))
add('docker_pat', 'dckr_pat_{0}', gen=lambda: r(27, B64U))
add('databricks', 'dapi{0}', gen=lambda: r(32, HEX))
add('atlassian', 'ATATT3xFfGF0{0}', gen=lambda: r(150, B64U))
add('linear', 'lin_api_{0}', gen=lambda: r(40))
add('supabase', 'sbp_{0}', gen=lambda: r(40, HEX))
add('postman', 'PMAK-{0}-{1}', 2, gen=lambda: r(24, HEX))
add('notion_secret_', 'secret_{0}', gen=lambda: r(43))
add('notion_ntn', 'ntn_{0}', gen=lambda: r(46))
add('telegram_bot', '123456789:AAH{0}', gen=lambda: r(32, B64U))
add('azure_conn', 'DefaultEndpointsProtocol=https;AccountName=acct;AccountKey={0}==;EndpointSuffix=core.windows.net',
    gen=lambda: r(84, B64))
add('azure_sas', 'https://acct.blob.core.windows.net/c/b?sv=2022-11-02&ss=b&sig={0}%3D&se=2027-01-01',
    gen=lambda: r(43, ALNUM))
add('vault_hvs', 'hvs.{0}', gen=lambda: r(90, B64U))
add('datadog_env', 'DD_API_KEY={0}', gen=lambda: r(32, HEX))
add('cloudflare_env', 'CF_API_TOKEN={0}', gen=lambda: r(40, B64U))
add('slack_webhook', 'https://hooks.slack.com/services/T0{0}/B0{1}/{2}', 3, gen=lambda: r(9, UPPER))
add('discord_webhook', 'https://discord.com/api/webhooks/123456789012345678/{0}', gen=lambda: r(68, B64U))
add('teams_webhook', 'https://outlook.office.com/webhook/{0}@{1}/IncomingWebhook/{2}/{3}', 4,
    gen=lambda: '-'.join(r(n, HEX) for n in (8, 4, 4, 4, 12)))
add('generic_webhook_path', 'POST https://ci.example.com/hooks/deploy/{0}', gen=lambda: r(40, HEX), doc='D3-unlabeled')
add('discord_bot_token', 'MTA{0}.GaBcDe.{1}', 2, gen=lambda: r(24, B64U), doc='D3-unlabeled')
add('github_webhook_secret', 'GITHUB_WEBHOOK_SECRET={0}', gen=lambda: r(32, HEX))
add('github_token_env', 'GITHUB_TOKEN={0}', gen=lambda: r(40))
add('ci_job_token', 'CI_JOB_TOKEN={0}', gen=lambda: r(20, B64U))

# ---- private key blocks ----
for kind in ('RSA PRIVATE KEY', 'EC PRIVATE KEY', 'OPENSSH PRIVATE KEY', 'PRIVATE KEY', 'ENCRYPTED PRIVATE KEY',
             'DSA PRIVATE KEY'):
    text, body = pem(kind)
    raw('pem_' + kind.split()[0].lower(), text, body[:3])
text, body = pem('PGP PRIVATE KEY BLOCK', 8)
raw('pgp_private_block', text.replace('\n', '\nVersion: x\n\n', 1) + '\n=abcd', body[:3])
text, body = pem('PRIVATE KEY')
raw('pem_no_end', text.split('-----END')[0], body[:3])
text, body = pem('RSA PRIVATE KEY')
raw('pem_crlf', text.replace('\n', '\r\n'), body[:3])
text, body = pem('RSA PRIVATE KEY')
raw('pem_json_escaped', json.dumps({'k': text}), body[:3])
text, body = pem('PRIVATE KEY')
raw('pem_indented_yaml', 'key: |\n' + '\n'.join('  ' + l for l in text.split('\n')), body[:3])
text, body = pem('PRIVATE KEY')
raw('pem_one_line_spaces', text.replace('\n', ' '), body[:3])
pk_body = r(64, B64)
raw('gcp_service_account_json',
    json.dumps({'type': 'service_account', 'private_key_id': r(40, HEX),
                'private_key': f'-----BEGIN PRIVATE KEY-----\n{pk_body}\n-----END PRIVATE KEY-----\n'}),
    [pk_body])
pk_b2 = r(64, B64)
raw('pem_after_text_same_line', f'key is -----BEGIN PRIVATE KEY----- {pk_b2} -----END PRIVATE KEY----- ok', [pk_b2])
kb = r(64, B64)
raw('putty_private_lines', f'PuTTY-User-Key-File-2: ssh-rsa\nEncryption: none\nPublic-Lines: 1\nabc\nPrivate-Lines: 1\n{kb}\nPrivate-MAC: x',
    [kb])

# ---- JWTs ----
j = jwt(); raw('jwt_bare', j, [j[10:]])
j = jwt(); raw('jwt_query', f'https://x.example.com/cb?access_token={j}&state=1', [j[10:]])
j = jwt(); raw('jwt_json_field', json.dumps({'id_token': j}), [j[10:]])
j = jwt(); raw('jwt_neutral_key_text', f'value {j} done', [j[10:]])
j5 = '.'.join('eyJ' + base64.urlsafe_b64encode(r(20).encode()).decode().rstrip('=') for _ in range(5))
raw('jwe_five_part', j5, [j5[10:30]])
unsigned = 'eyJhbGciOiJub25lIn0.' + base64.urlsafe_b64encode(('{"sub":"%s"}' % r(10)).encode()).decode().rstrip('=') + '.'
raw('jwt_unsigned_trailing_dot', unsigned, [unsigned.split('.')[1][:12]], doc='N-unsigned-claims')

# ---- URL credentials (any scheme) ----
add('url_postgres', 'postgres://app:{0}@db.internal:5432/app', gen=lambda: r(16))
add('url_postgresql_driver', 'postgresql+psycopg://app:{0}@db.internal/app', gen=lambda: r(16))
add('url_redis_emptyuser', 'redis://:{0}@cache.internal:6379/0', gen=lambda: r(16))
add('url_amqp', 'amqp://guest:{0}@mq.internal:5672//', gen=lambda: r(16))
add('url_mongodb_srv', 'mongodb+srv://admin:{0}@cluster0.example.mongodb.net/db', gen=lambda: r(16))
add('url_mysql', 'mysql://root:{0}@127.0.0.1/db', gen=lambda: r(16))
add('url_ftp', 'ftp://bob:{0}@ftp.example.com/pub', gen=lambda: r(16))
add('url_sftp', 'sftp://bob:{0}@host.example.com/data', gen=lambda: r(16))
add('url_smtp', 'smtp://mailer:{0}@smtp.example.com:587', gen=lambda: r(16))
add('url_ws', 'wss://bob:{0}@stream.example.com/feed', gen=lambda: r(16))
add('url_git_x_access_token', 'git clone https://x-access-token:{0}@github.com/o/r.git', gen=lambda: r(36))
add('url_gitlab_oauth2', 'https://oauth2:{0}@gitlab.com/o/r.git', gen=lambda: r(20, B64U))
add('url_token_userinfo_only', 'https://{0}@github.com/o/r.git', gen=lambda: r(40, HEX), doc='D3-unlabeled')
add('url_pw_with_at', 'postgres://app:p@{0}word@db.internal/app', gen=lambda: r(10))
add('url_pw_with_slash', 'redis://:ab/{0}cd@cache.internal', gen=lambda: r(10))
add('url_pw_pct_encoded', 'postgres://app:p%40{0}@db.internal/app', gen=lambda: r(10))
add('url_no_scheme', 'app:{0}@db.internal:5432/app', gen=lambda: r(16), doc='D2-schemeless')
add('url_scheme_relative', 'fetch //app:{0}@db.internal/app now', gen=lambda: r(16))
add('url_query_password', 'https://api.example.com/x?user=bob&password={0}&z=1', gen=lambda: r(16))
add('url_query_api_key', 'https://api.example.com/x?api_key={0}', gen=lambda: r(32))
add('url_query_apikey', 'https://api.example.com/x?apikey={0}', gen=lambda: r(32))
add('url_query_access_token', 'https://api.example.com/x?access_token={0}&a=1', gen=lambda: r(32))
add('url_query_token', 'https://api.example.com/x?token={0}', gen=lambda: r(32))
add('url_query_sig', 'https://api.example.com/x?sig={0}', gen=lambda: r(43))
add('url_query_key', 'https://maps.example.com/api?key={0}', gen=lambda: r(39))
add('url_query_client_secret', 'https://auth.example.com/t?client_id=a&client_secret={0}', gen=lambda: r(32))
add('url_aws_presigned', 'https://b.s3.amazonaws.com/o?X-Amz-Credential=AKIAX/2026/us-east-1/s3/aws4_request'
    '&X-Amz-Security-Token={1}&X-Amz-Signature={0}', 2, gen=lambda: r(64, HEX))
add('jdbc_password_param', 'jdbc:postgresql://db.internal/app?user=app&password={0}', gen=lambda: r(16))
add('odbc_password', 'Server=db;Database=app;User Id=app;Password={0};Encrypt=yes', gen=lambda: r(16))
add('odbc_pwd', 'Driver={{x}};Server=db;Uid=app;Pwd={0};', gen=lambda: r(16))
add('database_url_env', 'DATABASE_URL=postgres://app:{0}@db.internal/app', gen=lambda: r(16))

# ---- CLI flags ----
add('cli_password_space', 'tool login --password {0} --host x', gen=lambda: r(16))
add('cli_password_eq', 'tool login --password={0}', gen=lambda: r(16))
add('cli_docker_login_p', 'docker login -u bob -p {0} ghcr.io', gen=lambda: r(16))
add('cli_mysql_attached', 'mysql -u root -p{0} app', gen=lambda: r(16))
add('cli_mysql_long', 'mysql --password={0} app', gen=lambda: r(16))
add('cli_curl_u', 'curl -u bob:{0} https://x.example.com', gen=lambda: r(16))
add('cli_curl_user', 'curl --user bob:{0} https://x.example.com', gen=lambda: r(16))
add('cli_curl_proxy_user', 'curl --proxy-user bob:{0} https://x.example.com', gen=lambda: r(16))
add('cli_wget_password', 'wget --user=bob --password={0} https://x.example.com', gen=lambda: r(16))
add('cli_sshpass_p', 'sshpass -p {0} ssh bob@host', gen=lambda: r(16))
add('cli_pgpassword_env', 'PGPASSWORD={0} psql -h db app', gen=lambda: r(16))
add('cli_export_token', 'export TOKEN={0}', gen=lambda: r(24))
add('cli_token_flag', 'tool deploy --token {0}', gen=lambda: r(24))
add('cli_api_key_flag', 'tool deploy --api-key {0}', gen=lambda: r(24))
add('cli_secret_flag', 'tool deploy --secret {0}', gen=lambda: r(24))
add('cli_client_secret_eq', 'tool deploy --client-secret={0}', gen=lambda: r(24))
add('cli_docker_e', 'docker run -e AWS_SECRET_ACCESS_KEY={0} img', gen=lambda: r(40, B64))
add('cli_env_flag', 'tool run --env DB_PASSWORD={0}', gen=lambda: r(16))
add('cli_build_arg', 'docker build --build-arg NPM_TOKEN={0} .', gen=lambda: r(36))
add('cli_header_flag', "curl --header 'Authorization: Bearer {0}' https://x", gen=lambda: r(36))
add('cli_gh_with_token', 'gh auth login --with-token <<< {0}', gen=lambda: r(40, HEX), doc='D3-unlabeled')
add('cli_password_stdin_echo', 'echo {0} | docker login --password-stdin', gen=lambda: r(24), doc='D3-unlabeled')
add('cli_htpasswd_b', 'htpasswd -b .htpasswd bob {0}', gen=lambda: r(16))
add('cli_k8s_from_literal', 'kubectl create secret generic s --from-literal=password={0}', gen=lambda: r(16))
add('cli_terraform_var', "terraform apply -var 'db_password={0}'", gen=lambda: r(16))
add('cli_ansible_e', "ansible-playbook -e 'ansible_password={0}' site.yml", gen=lambda: r(16))
add('cli_openssl_pass', 'openssl enc -aes-256-cbc -pass pass:{0} -in a', gen=lambda: r(16))
add('cli_gpg_passphrase', 'gpg --batch --passphrase {0} -d f.gpg', gen=lambda: r(16))
add('cli_unzip_P', 'unzip -P {0} secret.zip', gen=lambda: r(16))
add('cli_ssh_keygen_N', 'ssh-keygen -t ed25519 -N {0} -f k', gen=lambda: r(16))

# ---- env assignments ----
add('env_api_key', 'API_KEY={0}', gen=lambda: r(32))
add('env_export_dq', 'export OPENAI_API_KEY="{0}"', gen=lambda: r(32))
add('env_export_sq', "export OPENAI_API_KEY='{0}'", gen=lambda: r(32))
add('env_db_password', 'DB_PASSWORD={0}', gen=lambda: r(16))
add('env_secret_key', 'SECRET_KEY={0}', gen=lambda: r(50))
add('env_passwd', 'MY_PASSWD={0}', gen=lambda: r(16))
add('env_client_secret', 'CLIENT_SECRET={0}', gen=lambda: r(32))
add('env_slack_bot_token', 'SLACK_BOT_TOKEN={0}', gen=lambda: r(40))
add('env_private_key_oneline', 'PRIVATE_KEY={0}', gen=lambda: r(60, B64))
add('env_password_spaced', 'password = {0}', gen=lambda: r(16))
add('env_PASSWORD_colon', 'PASSWORD: {0}', gen=lambda: r(16))
add('env_pass', 'DB_PASS={0}', gen=lambda: r(16))
add('env_passphrase', 'SSH_PASSPHRASE={0}', gen=lambda: r(16))
add('env_auth', 'AUTH={0}', gen=lambda: r(32))
add('env_creds', 'CREDS={0}', gen=lambda: r(32))
add('env_signing_key', 'SIGNING_KEY={0}', gen=lambda: r(32))
add('env_salt', 'PASSWORD_SALT={0}', gen=lambda: r(16))
add('env_bearer', 'BEARER={0}', gen=lambda: r(32))
add('env_session_id', 'SESSION_ID={0}', gen=lambda: r(32))
add('env_jwt', 'JWT={0}', gen=lambda: r(48, B64U))
add('env_block', 'A=1\nDB_USER=app\nDB_PASSWORD={0}\nAPI_TOKEN={1}\nDEBUG=true\n', 2, gen=lambda: r(20))
add('env_pw_short', 'pw={0}', gen=lambda: r(16))
add('env_pwd_short', 'pwd={0}', gen=lambda: r(16))

# ---- YAML / TOML / INI ----
add('yaml_password', 'db:\n  user: app\n  password: {0}\n  port: 5432', gen=lambda: r(16))
add('yaml_password_dq', 'password: "{0}"', gen=lambda: r(16))
add('yaml_password_sq', "password: '{0}'", gen=lambda: r(16))
add('yaml_block_scalar', 'api_key: |\n  {0}\n  {1}\nother: 1', 2, gen=lambda: r(48, B64))
add('yaml_folded_scalar', 'secret: >\n  {0}\n  {1}\nother: 1', 2, gen=lambda: r(40, B64))
add('yaml_k8s_env_name_value', 'env:\n  - name: DB_PASSWORD\n    value: {0}\n  - name: MODE\n    value: prod', gen=lambda: r(20))
add('yaml_flow_map', '{{password: {0}, user: bob}}', gen=lambda: r(16))
add('yaml_k8s_secret_data', 'kind: Secret\ndata:\n  password: {0}\n  user: Ym9i', gen=lambda: b64(r(12)))
add('yaml_ghactions_with_token', 'with:\n  token: {0}', gen=lambda: r(40))
add('toml_password', 'password = "{0}"', gen=lambda: r(16))
add('toml_token_literal', "token = '{0}'", gen=lambda: r(32))
add('toml_inline_table', "auth = {{ user = 'bob', password = '{0}' }}", gen=lambda: r(16))
add('toml_section_secrets', '[secrets]\ndb = "{0}"\n', gen=lambda: r(20))
add('toml_pass_key', 'db_pass = "{0}"', gen=lambda: r(16))
add('ini_password', '[db]\nuser=app\npassword={0}\n', gen=lambda: r(16))
add('docker_config_auth', '{{"auths":{{"ghcr.io":{{"auth":"{0}"}}}}}}', gen=lambda: b64('bob:' + r(30)))
add('netrc_inline', 'machine api.example.com login bob password {0}', gen=lambda: r(16))
add('netrc_multiline', 'machine api.example.com\n  login bob\n  password {0}\n', gen=lambda: r(16))
add('pgpass_line', 'localhost:5432:mydb:bob:{0}', gen=lambda: r(16), doc='D3-unlabeled')
add('py_kwarg_password', 'connect(host="h", user="app", password="{0}")', gen=lambda: r(16))
add('py_dict_password', "{{'password': '{0}', 'user': 'app'}}", gen=lambda: r(16))
add('py_assign_token', "token = '{0}'", gen=lambda: r(32))
add('md_bold_password', '**Password:** {0}', gen=lambda: r(16))
add('md_code_password', '`password`: `{0}`', gen=lambda: r(16))
add('md_table_password', '| password | {0} |', gen=lambda: r(16))
add('md_table_token', '| API token | {0} |', gen=lambda: r(32))
add('spaces_after_label_colon', 'password  :   {0}', gen=lambda: r(16))
add('tab_after_label', 'password=\t{0}', gen=lambda: r(16))
add('json_password', '{{"password": "{0}"}}', gen=lambda: r(16))
add('json_password_spaced', '{{ "password" :  "{0}" }}', gen=lambda: r(16))
add('json_secret_number_like', '{{"secret": {0}}}', gen=lambda: r(12, string.digits))
add('json_token', '{{"token": "{0}"}}', gen=lambda: r(32))
add('json_neutral_prefixed', '{{"value": "ghp_{0}"}}', gen=lambda: r(36))
add('json_auth_field', '{{"auth": "{0}"}}', gen=lambda: b64('bob:' + r(12)))
add('json_pass_field', '{{"pass": "{0}"}}', gen=lambda: r(16))
add('json_pwd_field', '{{"pwd": "{0}"}}', gen=lambda: r(16))
add('json_passphrase', '{{"passphrase": "{0}"}}', gen=lambda: r(16))
add('json_sessionid', '{{"sessionid": "{0}"}}', gen=lambda: r(32))
add('json_dsn', '{{"dsn": "postgres://app:{0}@db/x"}}', gen=lambda: r(16))
add('json_signature', '{{"signature": "{0}"}}', gen=lambda: r(43))
add('json_bearer_field', '{{"bearer": "{0}"}}', gen=lambda: r(40))
add('json_x_api_key', '{{"x-api-key": "{0}"}}', gen=lambda: r(40))

# ---- documented limitations (expected to leak; must not leak anywhere else) ----
add('DOC_unquoted_spaces', 'password=two {0} words', gen=lambda: r(12), doc='D1-spaces')
