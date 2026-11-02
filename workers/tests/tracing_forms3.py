"""Planted credential forms from the third #302 re-check. Every secret is a random fake from a fixed seed."""
import base64
import json
import random
import string

rng = random.Random(1101_2349)
ALNUM = string.ascii_letters + string.digits
HEX = '0123456789abcdef'
B64 = string.ascii_letters + string.digits + '+/'
B64U = string.ascii_letters + string.digits + '-_'
UPPER = string.ascii_uppercase + string.digits
PWCH = string.ascii_letters + string.digits + '!#%^*-_'

FORMS = []   # (name, text, [secrets], category)


def r(n, alpha=ALNUM):
    return ''.join(rng.choice(alpha) for _ in range(n))


def b64(s, urlsafe=False):
    f = base64.urlsafe_b64encode if urlsafe else base64.b64encode
    return f(s.encode() if isinstance(s, str) else s).decode()


def jwt():
    seg = lambda n: base64.urlsafe_b64encode(r(n).encode()).decode().rstrip('=')
    return 'eyJ' + seg(30) + '.' + 'eyJ' + seg(60) + '.' + seg(43)


def F(name, cat, text_fn):
    """text_fn(secrets-collector) -> text. Collector records secrets."""
    sec = []

    def S(v):
        sec.append(v)
        return v
    FORMS.append((name, text_fn(S), sec, cat))


# ================================================================ credential forms
F('azure_storage_connstr', 'cred', lambda S: (
    f"DefaultEndpointsProtocol=https;AccountName=prodlogs01;AccountKey={S(b64(r(64)))};EndpointSuffix=core.windows.net"))
F('azure_storage_connstr_appsettings', 'cred', lambda S: json.dumps({"ConnectionStrings": {"Blob":
    f"DefaultEndpointsProtocol=https;AccountName=st42;AccountKey={S(b64(r(64)))};EndpointSuffix=core.windows.net"}}, indent=2))
F('azure_storage_connstr_env', 'cred', lambda S: (
    f"export AZURE_STORAGE_CONNECTION_STRING='DefaultEndpointsProtocol=https;AccountName=st42;AccountKey={S(b64(r(64)))};BlobEndpoint=https://st42.blob.core.windows.net/'"))
F('azure_servicebus_sak', 'cred', lambda S: (
    f"Endpoint=sb://acme-prod.servicebus.windows.net/;SharedAccessKeyName=RootManageSharedAccessKey;SharedAccessKey={S(b64(r(32)))}"))
F('azure_sql_adonet', 'cred', lambda S: (
    f"Server=tcp:acme.database.windows.net,1433;Initial Catalog=orders;User ID=svc_orders;Password={S(r(18, PWCH))};Encrypt=True;TrustServerCertificate=False;"))
F('azure_sp_clientsecret_yaml', 'cred', lambda S: (
    f"clientId: 11111111-2222-3333-4444-555555555555\nclientSecret: {S(r(3) + '8Q~' + r(34, ALNUM + '_.~-'))}\ntenantId: 99999999-8888-7777-6666-555555555555"))
F('azure_sp_clientsecret_bare_env', 'cred', lambda S: (
    f"export ARM_CLIENT_SECRET={S(r(3) + '8Q~' + r(34, ALNUM + '_.~-'))}"))
F('azure_cosmos_connstr', 'cred', lambda S: (
    f"AccountEndpoint=https://acme.documents.azure.com:443/;AccountKey={S(b64(r(64)))};"))
gcp_pk_id = r(40, HEX)
gcp_body = '\\n'.join(r(64, B64) for _ in range(5))
F('gcp_sa_json_pretty', 'cred', lambda S: json.dumps({
    "type": "service_account", "project_id": "acme-prod", "private_key_id": S(gcp_pk_id),
    "private_key": "-----BEGIN PRIVATE KEY-----\n" + S(gcp_body.replace('\\n', '\n')[:128]) + "\n" +
                   '\n'.join(r(64, B64) for _ in range(4)) + "\n-----END PRIVATE KEY-----\n",
    "client_email": "svc@acme-prod.iam.gserviceaccount.com", "client_id": "123456789012345678901"}, indent=2))
F('gcp_sa_private_key_id_only', 'cred', lambda S: f'  "private_key_id": "{S(r(40, HEX))}",')
F('gcp_sa_private_key_only_oneline', 'cred', lambda S: (
    '"private_key": "-----BEGIN PRIVATE KEY-----\\n' + S(r(64, B64)) + '\\n' + r(64, B64) + '\\n' + r(44, B64) + '==\\n-----END PRIVATE KEY-----\\n",'))
F('gcp_sa_private_key_in_env', 'cred', lambda S: (
    'GOOGLE_PRIVATE_KEY="-----BEGIN PRIVATE KEY-----\\n' + S(r(64, B64)) + '\\n' + r(64, B64) + '\\n-----END PRIVATE KEY-----\\n"'))
F('k8s_secret_yaml_data', 'cred', lambda S: (
    "apiVersion: v1\nkind: Secret\nmetadata:\n  name: orders-db\n  namespace: prod\ntype: Opaque\ndata:\n"
    f"  username: {b64('orders_svc')}\n  password: {S(b64(r(20)))}\n  connection: {S(b64('postgres://orders_svc:' + r(14) + '@db:5432/orders'))}\n"))
F('k8s_secret_yaml_uncommon_keys', 'cred', lambda S: (
    "apiVersion: v1\nkind: Secret\nmetadata:\n  name: x\ndata:\n"
    f"  zorp: {S(b64(r(32)))}\n  quux.cfg: {S(b64(r(40)))}\n"))
F('k8s_secret_stringdata', 'cred', lambda S: (
    "apiVersion: v1\nkind: Secret\nmetadata:\n  name: x\nstringData:\n  config.yaml: |\n"
    f"    db:\n      host: db\n      pass: {S(r(20, PWCH))}\n"))
F('k8s_secret_tls', 'cred', lambda S: (
    "apiVersion: v1\nkind: Secret\ntype: kubernetes.io/tls\ndata:\n"
    f"  tls.crt: {b64(r(300))}\n  tls.key: {S(b64('-----BEGIN PRIVATE KEY-----' + r(200, B64)))}\n"))
F('k8s_secret_dockerconfigjson', 'cred', lambda S: (
    "kind: Secret\ntype: kubernetes.io/dockerconfigjson\ndata:\n  .dockerconfigjson: " +
    S(b64(json.dumps({"auths": {"registry.acme.io": {"username": "ci", "password": r(24), "auth": b64('ci:' + r(24))}}})))))
F('kubectl_get_secret_json', 'cred', lambda S: json.dumps({"apiVersion": "v1", "data": {
    "api-token": S(b64(r(36))), "ca.crt": b64(r(120))}, "kind": "Secret", "metadata": {"name": "t"}}, indent=2))
F('kubectl_get_secret_jsonpath_decoded', 'cred', lambda S: f"$ kubectl get secret orders-db -o jsonpath='{{.data.password}}' | base64 -d\n{S(r(24, PWCH))}")
F('pgpass_wildcard', 'cred', lambda S: f"*:*:*:postgres:{S(r(20, PWCH))}")
F('pgpass_host_db', 'cred', lambda S: f"db.internal.acme.io:5432:orders:orders_rw:{S(r(24, ALNUM))}")
F('pgpass_file_listing', 'cred', lambda S: (
    "$ cat ~/.pgpass\nlocalhost:5432:*:bob:" + S(r(16, ALNUM)) + "\nreplica.acme.io:5432:orders:ro:" + S(r(16, ALNUM))))
F('docker_config_auths', 'cred', lambda S: json.dumps({"auths": {"ghcr.io": {"auth": S(b64('rob:ghp_' + r(36)))},
    "https://index.docker.io/v1/": {"auth": S(b64('rob:' + r(22)))}}}, indent=2))
F('docker_config_identitytoken', 'cred', lambda S: json.dumps({"auths": {"registry.acme.io": {"identitytoken": S(r(60, B64U))}}}))
F('docker_config_registrytoken', 'cred', lambda S: json.dumps({"auths": {"gcr.io": {"registrytoken": S(r(48, B64U))}}}))
F('docker_config_cat_session', 'cred', lambda S: "$ cat ~/.docker/config.json\n{\n\t\"auths\": {\n\t\t\"quay.io\": {\n\t\t\t\"auth\": \"" + S(b64('rob+robot:' + r(60, UPPER))) + "\"\n\t\t}\n\t}\n}")
F('github_pat_fine_grained_bare', 'cred', lambda S: S('github_pat_' + r(22) + '_' + r(59)))
F('github_pat_fine_grained_in_prose', 'cred', lambda S: f"Cloned using {S('github_pat_' + r(22, ALNUM) + '_' + r(59, ALNUM))} as the token and it worked.")
F('github_pat_in_git_remote', 'cred', lambda S: f"origin\thttps://rob:{S('github_pat_' + r(22) + '_' + r(59))}@github.com/acme/repo.git (fetch)")
F('glpat_bare', 'cred', lambda S: f"{S('glpat-' + r(20, ALNUM + '-_'))}")
F('glpat_in_header', 'cred', lambda S: f"PRIVATE-TOKEN: {S('glpat-' + r(20, ALNUM + '-_'))}")
F('glpat_in_clone_url', 'cred', lambda S: f"git clone https://oauth2:{S('glpat-' + r(20, ALNUM + '-_'))}@gitlab.acme.io/g/p.git")
F('glpat_routable_new_format', 'cred', lambda S: f"{S('glpat-' + r(27, ALNUM + '-_') + '.01.' + r(9, ALNUM))}")
F('xoxe_dot_token', 'cred', lambda S: S('xoxe.xoxp-1-' + r(150, ALNUM + '-')))
F('xoxe_refresh', 'cred', lambda S: S('xoxe-1-' + r(120, ALNUM + '-')))
F('xoxe_in_json', 'cred', lambda S: json.dumps({"ok": True, "access_token": S('xoxe.xoxp-1-' + r(120, ALNUM + '-')), "refresh_token": S('xoxe-1-' + r(100, ALNUM + '-'))}))
akid = lambda: 'AKIA' + r(16, UPPER)
F('akia_then_secret_next_line', 'cred', lambda S: f"{akid()}\n{S(r(40, B64))}")
F('akia_then_secret_next_line_prose', 'cred', lambda S: f"Here are the creds you asked for:\n{akid()}\n{S(r(40, B64))}\nUse them for the s3 sync.")
F('akia_colon_secret_csv', 'cred', lambda S: f"Access key ID,Secret access key\n{akid()},{S(r(40, B64))}")
F('akia_secret_aws_iam_csv_download', 'cred', lambda S: f"User name,Password,Access key ID,Secret access key,Console login link\nci-bot,,{akid()},{S(r(40, B64))},https://123456789012.signin.aws.amazon.com/console")
F('akia_secret_python_call', 'cred', lambda S: f"s3 = boto3.client('s3', '{akid()}', '{S(r(40, B64))}')")
F('akia_secret_two_lines_with_gap', 'cred', lambda S: f"{akid()}\n\n{S(r(40, B64))}")
F('akia_secret_aws_sts_json', 'cred', lambda S: json.dumps({"Credentials": {"AccessKeyId": 'ASIA' + r(16, UPPER), "SecretAccessKey": S(r(40, B64)),
    "SessionToken": S('IQoJb3JpZ2luX2VjE' + r(300, B64)), "Expiration": "2026-11-01T22:00:00Z"}}, indent=2))
F('auth_token_hex40', 'cred', lambda S: f"Authorization: token {S(r(40, HEX))}")
F('auth_token_curl_dq', 'cred', lambda S: f'curl -s -H "Authorization: token {S(r(40, HEX))}" https://api.github.com/user')
F('auth_token_ghp', 'cred', lambda S: f"Authorization: token {S('ghp_' + r(36))}")
F('auth_token_json_headers', 'cred', lambda S: json.dumps({"headers": {"Authorization": "token " + S(r(40, HEX)), "Accept": "application/json"}}))
F('auth_token_python_requests', 'cred', lambda S: f"requests.get(url, headers={{'Authorization': 'token {S(r(40, HEX))}'}})")
F('auth_token_wire_http', 'cred', lambda S: f"GET /user HTTP/1.1\r\nHost: api.github.com\r\nAuthorization: token {S(r(40, HEX))}\r\nUser-Agent: x\r\n\r\n")
F('mongodb_srv_basic', 'cred', lambda S: f"mongodb+srv://orders_user:{S(r(20))}@cluster0.ab1cd.mongodb.net/orders?retryWrites=true&w=majority")
F('mongodb_srv_pct', 'cred', lambda S: f"mongodb+srv://admin:{S('p%40' + r(12) + '%2F' + r(6))}@cluster0.ab1cd.mongodb.net/?appName=x")
F('mongodb_srv_env_quoted', 'cred', lambda S: f'MONGO_URI="mongodb+srv://u:{S(r(22, PWCH))}@c0.mongodb.net/db"')
F('mongodb_srv_json', 'cred', lambda S: json.dumps({"db": {"uri": f"mongodb+srv://svc:{S(r(20))}@c0.mongodb.net/db"}}))
F('mongodb_srv_markdown_link', 'cred', lambda S: f"See [the cluster](mongodb+srv://svc:{S(r(20))}@c0.mongodb.net/db) for details.")
F('mongodb_plain_replica', 'cred', lambda S: f"mongodb://root:{S(r(20))}@h1:27017,h2:27017,h3:27017/admin?replicaSet=rs0")

# ================================================================ token placement
F('jwt_in_url_path', 'place', lambda S: f"https://api.acme.io/v1/verify/{S(jwt())}/status")
F('jwt_in_url_query_unlabeled', 'place', lambda S: f"https://app.acme.io/cb?t={S(jwt())}&next=/home")
F('jwt_in_url_fragment', 'place', lambda S: f"https://app.acme.io/cb#id_token={S(jwt())}&state=xyz")
F('jwt_in_url_magic_link', 'place', lambda S: f"https://app.acme.io/auth/magic/{S(jwt())}")
F('jwt_in_log_line', 'place', lambda S: f"2026-11-01T22:01:02Z INFO  req GET /api/me from=10.0.0.4 jwt={S(jwt())} ms=14")
F('jwt_two_segments_truncated', 'place', lambda S: f"curl https://x.acme.io/?auth={S('eyJ' + r(36, B64U) + '.eyJ' + r(60, B64U))}")
F('jwt_in_websocket_url', 'place', lambda S: f"wss://rt.acme.io/socket?token={S(jwt())}&vsn=2.0.0")
F('presigned_s3_v4', 'place', lambda S: (
    f"https://acme-prod.s3.amazonaws.com/exports/out.csv?X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Credential={akid()}%2F20261101%2Fus-east-1%2Fs3%2Faws4_request"
    f"&X-Amz-Date=20261101T220000Z&X-Amz-Expires=3600&X-Amz-SignedHeaders=host&X-Amz-Signature={S(r(64, HEX))}"))
F('presigned_s3_security_token', 'place', lambda S: (
    f"https://acme-prod.s3.amazonaws.com/o?X-Amz-Security-Token={S(b64(r(200), True).rstrip('='))}&X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Signature={S(r(64, HEX))}"))
F('presigned_gcs_v4', 'place', lambda S: (
    "https://storage.googleapis.com/acme-bucket/obj.bin?X-Goog-Algorithm=GOOG4-RSA-SHA256&X-Goog-Credential=svc%40acme-prod.iam.gserviceaccount.com%2F20261101%2Fauto%2Fstorage%2Fgoog4_request"
    f"&X-Goog-Date=20261101T220000Z&X-Goog-Expires=900&X-Goog-SignedHeaders=host&X-Goog-Signature={S(r(256, HEX))}"))
F('presigned_gcs_v2', 'place', lambda S: f"https://storage.googleapis.com/b/o?GoogleAccessId=svc%40p.iam.gserviceaccount.com&Expires=1793600000&Signature={S(r(80, B64).replace('+', '%2B').replace('/', '%2F') + '%3D')}")
F('presigned_cloudfront', 'place', lambda S: f"https://d1abc.cloudfront.net/v/x.mp4?Policy={S(r(120, B64U))}&Signature={S(r(80, B64U) + '~')}&Key-Pair-Id=APKAEXAMPLE12345")
F('presigned_azure_sas_sig', 'place', lambda S: f"https://st42.blob.core.windows.net/c/b.zip?sv=2022-11-02&se=2026-12-01T00%3A00%3A00Z&sr=b&sp=r&sig={S(b64(r(32)).replace('+', '%2B').replace('/', '%2F').replace('=', '%3D'))}")
F('cookie_session_header', 'place', lambda S: f"Cookie: sessionid={S(r(32, HEX))}; csrftoken={S(r(64))}; theme=dark")
F('cookie_connect_sid', 'place', lambda S: f"Cookie: connect.sid=s%3A{S(r(32))}.{S(r(43, B64U))}")
F('cookie_curl_b', 'place', lambda S: f"curl -b 'session={S(r(40, B64U))}' https://app.acme.io/api/me")
F('cookie_curl_cookie_flag', 'place', lambda S: f'curl --cookie "JSESSIONID={S(r(32, UPPER))}" https://app.acme.io/')
F('cookie_nextauth', 'place', lambda S: f"Cookie: __Secure-next-auth.session-token={S(r(180, B64U))}")
F('cookie_flask_session', 'place', lambda S: f"Set-Cookie: session={S('eyJ' + r(60, B64U) + '.' + r(6, B64U) + '.' + r(27, B64U))}; HttpOnly; Path=/")
F('cookie_browser_export_json', 'place', lambda S: json.dumps([{"name": "sid", "value": S(r(48, B64U)), "domain": ".acme.io", "path": "/", "httpOnly": True, "secure": True}]))
F('cookie_netscape_txt', 'place', lambda S: f".acme.io\tTRUE\t/\tTRUE\t1893456000\tsession_id\t{S(r(40, HEX))}")
F('cookie_python_dict', 'place', lambda S: f"requests.get(u, cookies={{'sessionid': '{S(r(32, HEX))}'}})")
F('cookie_wire_multi', 'place', lambda S: f"GET /a HTTP/1.1\r\nHost: h\r\nCookie: _gh_sess={S(r(120, B64U))}; user_session={S(r(43, B64U))}\r\n\r\n")

# ================================================================ variable names
F('env_french_mot_de_passe', 'names', lambda S: f"export MOT_DE_PASSE={S(r(28))}")
F('env_german_passwort', 'names', lambda S: f"export DB_PASSWORT={S(r(28))}")
F('env_german_geheim', 'names', lambda S: f"export GEHEIMNIS={S(r(32))}")
F('env_german_schluessel', 'names', lambda S: f"SCHLUESSEL={S(r(40, B64))}")
F('env_spanish_contrasena', 'names', lambda S: f"export CONTRASENA_BD={S(r(26))}")
F('env_portuguese_senha', 'names', lambda S: f"export SENHA={S(r(26))}")
F('env_italian_parola_chiave', 'names', lambda S: f"export PAROLA_CHIAVE={S(r(26))}")
F('env_russian_cyrillic', 'names', lambda S: f"export ПАРОЛЬ={S(r(26))}")
F('env_chinese_label', 'names', lambda S: f"密码={S(r(26))}")
F('env_japanese_label', 'names', lambda S: f"パスワード: {S(r(26))}")
F('env_uncommon_hex64', 'names', lambda S: f"export ZARQ_SEED={S(r(64, HEX))}")
F('env_uncommon_hex32', 'names', lambda S: f"export FROBNITZ={S(r(32, HEX))}")
F('env_uncommon_b64_32', 'names', lambda S: f"export QUUX_BLOB={S(b64(r(32)))}")
F('env_uncommon_alnum40', 'names', lambda S: f"XYZZY_VALUE={S(r(40))}")
F('env_uncommon_urlsafe43', 'names', lambda S: f"export WIBBLE={S(r(43, B64U))}")
F('env_uncommon_in_docker_e', 'names', lambda S: f"docker run -e MYAPP_BLORP={S(r(48, HEX))} -e MODE=prod acme/app:1.2")
F('env_uncommon_dotenv_quoted', 'names', lambda S: f'GLORP_ID="{S(r(36))}"')
F('env_uncommon_yaml_env', 'names', lambda S: f"env:\n  - name: NIMBUS_Z\n    value: \"{S(r(36))}\"")
F('env_rails_secret_key_base', 'names', lambda S: f"SECRET_KEY_BASE={S(r(128, HEX))}")
F('env_hmac_seed_hex64', 'names', lambda S: f"HMAC_SEED={S(r(64, HEX))}")
F('env_openssl_rand_hex_output', 'names', lambda S: f"$ openssl rand -hex 32\n{S(r(64, HEX))}")
F('env_openssl_rand_base64_output', 'names', lambda S: f"$ openssl rand -base64 32\n{S(b64(r(32)))}")
F('env_pwgen_output', 'names', lambda S: f"$ pwgen -s 24 1\n{S(r(24))}")
F('env_uuid_labeled_heroku', 'names', lambda S: f"HEROKU_API_KEY={S('%s-%s-%s-%s-%s' % (r(8, HEX), r(4, HEX), r(4, HEX), r(4, HEX), r(12, HEX)))}")
F('env_uuid_json_token', 'names', lambda S: json.dumps({"token": S('%s-%s-%s-%s-%s' % (r(8, HEX), r(4, HEX), r(4, HEX), r(4, HEX), r(12, HEX)))}))
F('env_camel_proxyPassword', 'names', lambda S: f"proxyPassword: {S(r(20, PWCH))}")
F('env_camel_accessKeySecret', 'names', lambda S: f'accessKeySecret = "{S(r(30))}"')
F('env_kebab_api_secret', 'names', lambda S: f"api-secret: {S(r(30))}")
F('env_ansible_become_pass', 'names', lambda S: f"ansible_become_pass={S(r(18, PWCH))}")
F('env_ansible_ssh_pass_inventory', 'names', lambda S: f"web1 ansible_host=10.0.0.5 ansible_user=deploy ansible_ssh_pass={S(r(18, PWCH))} ansible_port=22")
F('env_ansible_vault_blob', 'names', lambda S: "$ANSIBLE_VAULT;1.1;AES256\n" + '\n'.join(S(r(80, HEX)) for _ in range(2)))

# ================================================================ encodings
F('b64_in_json_password_equals', 'enc', lambda S: json.dumps({"blob": S(b64('password=' + r(18, PWCH)))}))
F('b64_in_json_creds_pair', 'enc', lambda S: json.dumps({"payload": S(b64(f'{akid()}:{r(40, B64)}'))}))
F('b64_in_json_pem', 'enc', lambda S: json.dumps({"content": S(b64('-----BEGIN RSA PRIVATE KEY-----\n' + r(300, B64) + '\n-----END RSA PRIVATE KEY-----'))}))
F('b64_in_json_dotenv', 'enc', lambda S: json.dumps({"encoding": "base64", "content": S(b64('STRIPE_KEY=sk_live_' + r(24) + '\nDB_PASS=' + r(16) + '\n'))}))
F('hex_in_json_secret_bytes', 'enc', lambda S: json.dumps({"id": 7, "v": S(r(64, HEX)), "ts": 1793600000}))
F('hex_in_json_utf8_of_pw', 'enc', lambda S: json.dumps({"data": S(('password=' + r(16)).encode().hex())}))
F('hex_in_json_nested', 'enc', lambda S: json.dumps({"result": {"rows": [{"c0": "u1", "c1": S(r(32, HEX))}]}}))
F('urlenc_in_json_pw', 'enc', lambda S: json.dumps({"body": "username=bob&password=" + S('%40' + r(14) + '%21') + "&grant_type=password"}))
F('b64_basic_in_json_nolabel', 'enc', lambda S: json.dumps({"h": ["Basic " + S(b64('svc:' + r(20)))]}))
F('rot_json_in_json_in_json', 'enc', lambda S: json.dumps({"outer": json.dumps({"inner": json.dumps({"api_secret_x": S(r(30))})})}))
F('json_array_of_pairs', 'enc', lambda S: json.dumps({"env": [["PORT", "8080"], ["ZZ_OPAQUE", S(r(36))]]}))
F('json_kv_objects_name_value', 'enc', lambda S: json.dumps({"variables": [{"key": "DB_PASSWORD", "value": S(r(20, PWCH))}, {"key": "REGION", "value": "us-east-1"}]}))
F('json_kv_objects_uncommon_name', 'enc', lambda S: json.dumps({"variables": [{"name": "BLEEP", "value": S(r(36))}]}))
F('unicode_escaped_json', 'enc', lambda S: '{"p\\u0061ssword": "' + S(r(20)) + '"}')
F('yaml_anchors_secret', 'enc', lambda S: f"defaults: &d\n  passwd: {S(r(20, PWCH))}\nprod:\n  <<: *d")
F('escaped_quotes_in_text', 'enc', lambda S: 'The output was \\"password\\": \\"' + S(r(20)) + '\\", as expected.')
F('python_repr_namespace', 'enc', lambda S: f"Namespace(host='h', user='u', password='{S(r(18, PWCH))}', verbose=True)")
F('python_connect_kwargs', 'enc', lambda S: f"psycopg2.connect(host='h', dbname='d', user='u', password='{S(r(18, PWCH))}')")
F('go_struct_dump', 'enc', lambda S: f"{{Host:db Port:5432 User:app Password:{S(r(18, ALNUM))} SSLMode:require}}")
F('rust_debug_dump', 'enc', lambda S: f'Config {{ host: "db", api_key: "{S(r(32))}", retries: 3 }}')
F('xml_password_element', 'enc', lambda S: f"<server><id>nexus</id><username>deploy</username><password>{S(r(20, PWCH))}</password></server>")
F('xml_add_key_value', 'enc', lambda S: f'<add key="ApiKey" value="{S(r(32))}" />')
F('xml_hibernate_property', 'enc', lambda S: f'<property name="hibernate.connection.password">{S(r(18, PWCH))}</property>')
F('csv_username_password', 'enc', lambda S: f"username,password,role\nbob,{S(r(16, PWCH))},admin")
F('sql_create_user', 'enc', lambda S: f"CREATE USER orders_svc WITH PASSWORD '{S(r(20, PWCH))}';")
F('sql_identified_by', 'enc', lambda S: f"CREATE USER 'app'@'%' IDENTIFIED BY '{S(r(20, PWCH))}';")
F('powershell_securestring', 'enc', lambda S: f"$p = ConvertTo-SecureString '{S(r(18, PWCH))}' -AsPlainText -Force")
F('net_use_cmd', 'enc', lambda S: f"net use \\\\srv\\share /user:CORP\\bob {S(r(16, PWCH))}")
F('prose_password_is', 'enc', lambda S: f"The password for the staging database is {S(r(16, PWCH))} - please rotate it.")
F('prose_code_comment', 'enc', lambda S: f"# temp admin password: {S(r(16, PWCH))}  (remove before merge)")
F('prose_stacktrace', 'enc', lambda S: f"psycopg2.OperationalError: FATAL: password authentication failed for user \"app\" (password used: {S(r(16, PWCH))})")
F('redis_cli_a', 'enc', lambda S: f"redis-cli -h cache.acme.io -a {S(r(24))} ping")
F('vault_login_cli', 'enc', lambda S: f"vault login {S('hvs.' + r(24, ALNUM + '_-'))}")
F('git_credential_protocol', 'enc', lambda S: f"protocol=https\nhost=github.com\nusername=rob\npassword={S('ghp_' + r(36))}")
F('gradle_properties', 'enc', lambda S: f"systemProp.https.proxyPassword={S(r(18, PWCH))}\nsigning.password={S(r(18, PWCH))}")
F('terraform_tfvars', 'enc', lambda S: f'db_password = "{S(r(20, PWCH))}"\nregion = "us-east-1"')
F('terraform_tfstate_secret', 'enc', lambda S: json.dumps({"attributes": {"secret_string": S(r(30)), "arn": "arn:aws:secretsmanager:us-east-1:1:secret:x"}}, indent=2))
F('new_prefix_dckr_pat', 'prefix', lambda S: f"docker login -u rob --password-stdin <<< {S('dckr_pat_' + r(27, ALNUM + '_-'))}")
F('new_prefix_dop_v1', 'prefix', lambda S: S('dop_v1_' + r(64, HEX)))
F('new_prefix_shpat', 'prefix', lambda S: S('shpat_' + r(32, HEX)))
F('new_prefix_lin_api', 'prefix', lambda S: S('lin_api_' + r(40)))
F('new_prefix_pmak', 'prefix', lambda S: S('PMAK-' + r(24, HEX) + '-' + r(34, HEX)))
F('new_prefix_r8_replicate', 'prefix', lambda S: S('r8_' + r(37)))
F('new_prefix_groq_gsk', 'prefix', lambda S: S('gsk_' + r(52)))
F('new_prefix_perplexity', 'prefix', lambda S: S('pplx-' + r(48, HEX)))
F('new_prefix_tfc_atlasv1', 'prefix', lambda S: S(r(14) + '.atlasv1.' + r(66, ALNUM + '-_')))
F('new_prefix_telegram_in_url', 'prefix', lambda S: f"https://api.telegram.org/bot{S(str(rng.randrange(10**9, 10**10)) + ':AA' + r(33, ALNUM + '-_'))}/getMe")
F('new_prefix_discord_bot', 'prefix', lambda S: S('MTA' + r(21, B64U) + '.' + r(6, B64U) + '.' + r(27, B64U)))
F('new_prefix_openai_proj_long', 'prefix', lambda S: S('sk-proj-' + r(120, ALNUM + '-_')))
F('new_prefix_anthropic_api03', 'prefix', lambda S: S('sk-ant-api03-' + r(93, ALNUM + '-_') + 'AA'))
F('new_prefix_sentry_auth', 'prefix', lambda S: S('sntrys_' + b64(r(80), True).rstrip('=')))
F('new_prefix_cloudflare_unlabeled', 'prefix', lambda S: f"curl -H 'Authorization: Bearer {S(r(40, ALNUM + '-_'))}' https://api.cloudflare.com/client/v4/user/tokens/verify")
F('new_prefix_figma', 'prefix', lambda S: S('figd_' + r(40, ALNUM + '-_')))
F('new_prefix_airtable_pat', 'prefix', lambda S: S('pat' + r(14) + '.' + r(64, HEX)))
F('new_prefix_ssh_openssh_oneline_json', 'prefix', lambda S: json.dumps({"deploy_key": "-----BEGIN OPENSSH PRIVATE KEY-----\n" + S(r(70, B64)) + "\n" + r(70, B64) + "\n-----END OPENSSH PRIVATE KEY-----\n"}))
F('pem_body_only_no_header', 'prefix', lambda S: '\n'.join(S(r(64, B64)) for _ in range(5)) + '\n-----END RSA PRIVATE KEY-----')
F('pem_head_c_cutoff', 'prefix', lambda S: '$ head -c 300 key.pem\n-----BEGIN RSA PRIVATE KEY-----\n' + '\n'.join(S(r(64, B64)) for _ in range(3)) + '\n' + S(r(20, B64)))
F('sentry_dsn_old_secret', 'prefix', lambda S: f"https://{r(32, HEX)}:{S(r(32, HEX))}@o1.ingest.sentry.io/4506")
F('twilio_client_call', 'prefix', lambda S: f'client = Client("AC{r(32, HEX)}", "{S(r(32, HEX))}")')
