"""Observational MLflow capture with bounded, redacted content and detached export."""
from collections import deque
from contextlib import contextmanager
from contextvars import ContextVar
import json
import logging
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import threading
import tomllib


active = ContextVar('tl3_trace_capture', default=None)
PAYLOAD_BYTES = 65536
TRACE_BYTES = 1024 * 1024
STRING_CHARS = 8192
ITEM_BYTES = 16384
# Text kept beyond each cut, so a label or prefix outside the shown window still
# redacts the value inside it.
CONTEXT = 1024
# Every span keeps up to this much content before the newest spans take the rest.
FLOOR_BYTES = 4096
# Spans beyond the first and last of these are omitted from very long runs.
FIRST_SPANS, LAST_SPANS = 1024, 3072
# Unredacted, bounded content held until the run ends. Older spans shrink to the
# floor beyond this, which the final allocation would give them anyway.
SHAPED_BYTES = 4 * TRACE_BYTES
TOKEN_FIELDS = ('input_tokens', 'output_tokens', 'total_tokens',
                'cache_read_input_tokens', 'cache_creation_input_tokens')
REDACTED = '[REDACTED]'
LEAD_NOTE = '[truncated: earlier text omitted]\n'

# A label names a secret when one of its words does. Words are split at
# underscores, hyphens, dots and camel case, so `input_tokens` and `TokenBucket`
# don't count as `token`, while `PGPASSWORD` and `_authToken` do.
LABEL_WORDS = re.compile(r'[A-Z]+(?![a-z])|[A-Z]?[a-z]+|\d+')
SECRET_PARTS = ('password', 'passwd', 'passphrase', 'secret', 'credential', 'apikey', 'privatekey',
                'accesskey', 'secretkey', 'accesstoken', 'authtoken', 'refreshtoken', 'sessionid',
                'bearer', 'cookie', 'authorization', 'pgpass')
SECRET_WORDS = {'cred', 'creds', 'jwt', 'signature', 'passcode'}
# Short or common words that also name ordinary things. Their values must look random.
WEAK_WORDS = {'key', 'session', 'pass', 'auth', 'pwd', 'pw', 'sig'}
KEY_QUALIFIERS = {'api', 'private', 'access', 'secret', 'signing', 'account', 'master', 'encryption',
                  'subscription', 'model', 'app', 'client', 'license', 'auth', 'webhook', 'ssh', 'deploy',
                  'service', 'consumer', 'storage', 'session'}
# Words that make a label describe a secret rather than hold one.
NOT_SECRET = {'count', 'counts', 'counter', 'budget', 'limit', 'limits', 'len', 'length', 'size', 'max',
              'min', 'type', 'types', 'kind', 'url', 'uri', 'endpoint', 'path', 'paths', 'file', 'files',
              'dir', 'name', 'names', 'bucket', 'details', 'usage', 'metadata', 'estimate', 'ttl', 'expiry',
              'expires', 'expiration', 'lifetime', 'field', 'fields', 'prefix', 'regex', 'pattern', 'policy',
              'scope', 'scopes', 'store', 'manager', 'provider', 'rate', 'window', 'version', 'hint', 'label',
              'ref', 'reset', 'required', 'enabled', 'rotation', 'format', 'encoding', 'mode', 'method',
              'timeout', 'interval', 'stdin', 'env', 'index', 'offset', 'error', 'errors', 'message',
              'prompt', 'description', 'help', 'example', 'placeholder', 'header', 'is', 'has', 'load',
              'helper'}
# Values that say they are not real.
PLACEHOLDERS = {'fake', 'fixture', 'test', 'dummy', 'example', 'unused', 'placeholder', 'sample', 'changeme',
                'redacted', 'disposable', 'mock', 'stub', 'invalid'}
KEYWORDS = {'lambda', 'None', 'True', 'False', 'null', 'nil', 'true', 'false', 'none', 'undefined', 'self',
            'this', 'cls', 'def', 'return', 'if', 'else', 'elif', 'not', 'and', 'or', 'in', 'is', 'await',
            'async', 'new', 'yield', 'pass', 'fn', 'let', 'const', 'var', 'mut', 'ref', 'impl', 'match',
            'Some', 'Ok', 'Err', 'required', 'optional', 'continue', 'break', 'raise', 'try', 'except',
            'finally', 'with', 'for', 'while', 'del', 'global', 'nonlocal', 'assert', 'import', 'from', 'class',
            'loop', 'unsafe', 'use', 'mod', 'pub', 'static', 'enum', 'struct', 'trait', 'where', 'dyn', 'move'}
TYPE_NAMES = {'str', 'bytes', 'int', 'float', 'bool', 'dict', 'list', 'tuple', 'set', 'object', 'Any',
              'Optional', 'Union', 'Literal', 'String', 'Vec', 'Option', 'Box', 'Self', 'usize', 'u8', 'u16',
              'u32', 'u64', 'i32', 'i64', 'string', 'number', 'boolean', 'any', 'unknown', 'void', 'Path',
              'PathBuf', 'SecretStr', 'SecretString', 'SecretBytes', 'expr', 'ident', 'ty', 'tt', 'pat',
              'path', 'literal', 'block', 'stmt', 'item', 'meta', 'vis', 'lifetime'}
REFERENCE = re.compile(r'[${<]|%[({A-Za-z_]|\*{3}|\.{3}')
TEMPLATE = re.compile(r'\$\{|\{\{|\$\(|\{[\w.]*\}|%\(\w+\)s|::')
SCHEME_NAME = re.compile(r'(?i)bearer|basic|token|digest|negotiate|ntlm|apikey|aws4-hmac-sha256')
# A path segment that reads as a name, which a random token rarely contains.
PATH_SEGMENT = re.compile(r'[a-z]{4,}|[a-z]+(?:[-_][a-z0-9]+)+')
VERSION = re.compile(r'\d{1,3}|\d{1,4}(?:\.\d{1,4}){1,3}')
# A variable or attribute name passed as a value, as in `token = runtime`.
CODE_NAME = re.compile(r'[a-z_][a-z0-9_]*+|[a-z]++(?:[A-Z][a-z]{2,}+\d*+)++')
PATHLIKE = re.compile(r'(?:~|\.{1,2})?/[\w.@-]*+(?:/[\w.@-]*+)*+')
DOTTED = re.compile(r'[A-Za-z_]\w*+(?:\.[A-Za-z_]\w*+)++')
NUMBER = re.compile(r'-?\d{1,6}(?:\.\d+)?|-?\d+\.\d+')
IDENTIFIER = re.compile(r'[A-Za-z_]\w*+')
WORD = re.compile(r'[A-Za-z][a-z]*+')

# JSON inside JSON puts a literal backslash before each quote and whitespace escape.
SPACE = r'(?:[ \t\r\n]|\\+[nrt])*+'
QUOTED = r"""(?:\\+"[^"\r\n]*+"|"(?:[^"\\\r\n]|\\.)*+"|\\*'[^'\r\n]*+'|`[^`\r\n]*+`)"""
# Values are bounded lookaheads: a rejected value is scanned again for labels
# of its own, and a run of labels costs at most the bound each.
BOUNDED = (r"""(?:[rbfuRBFU]{1,2}(?=["'])|)(?:\\+"[^"\r\n]{0,4096}+"|"(?:[^"\\\r\n]|\\.){0,4096}+"""
           r""""|\\*'[^'\r\n]{0,4096}+'|`[^`\r\n]{0,4096}+`)""")
# Identifiers are matched possessively and judged in Python, so a long run of
# word characters is scanned once.
LABELLED = re.compile(
    r'(?<![\w.$\\-])(?P<q>\\*["\'`]|[*_]{1,2})?\$?(?P<l>[A-Za-z_][\w.-]*+)(?:\\*["\'`]|\*{1,2})?[ \t]*+'
    r'(?P<a>:(?![:=])|=(?![=>~]))[*_]{0,2}(?=' + SPACE + '(?P<v>' + BOUNDED
    + r"""|[^\s"'`\\,;}&|\])<>]{1,512}+))""")
FLAG = re.compile(r'(?<![\w-])--?(?P<l>[A-Za-z][\w-]*+)(?=(?:=|[ \t]++)(?P<v>' + BOUNDED
                  + r"""|[^\s"'`]{1,512}+))""")
TABLE = re.compile(r'(?m)(?:^[ \t]*+|(?<![\w|]))\|[ \t]++(?P<l>[^|\n]{1,64}?)[ \t]++\|[ \t]++'
                   r'(?P<v>[^|\s][^|\n]{0,254}?)[ \t]++(?=\|)')
BLOCK_SCALAR = re.compile(
    r'(?m)^(?P<i>[ \t]*+)(?:-[ \t]++)?["\']?(?P<l>[\w.-]++)["\']?[ \t]*+:[ \t]*+[|>][-+0-9]*+[ \t]*+'
    r'(?:#[^\n]*+)?\r?\n(?P<v>(?:(?P=i)[ \t]++[^\n]*+(?:\n|\Z)|[ \t]*+\r?\n)++)')
NAME_VALUE = re.compile(r'\bname:[ \t]*+["\']?(?P<l>[\w.-]++)["\']?[ \t]*+\r?\n[ \t]*+(?:-[ \t]++)?'
                        r'value:[ \t]*+(?P<v>' + QUOTED + r'|[^\s#]++)')
SECTION = re.compile(r'(?m)(?:^[ \t]*+|(?<![\w\]]))\[\[?(?P<l>[^\]\n"\',]{1,64})\]\]?[ \t]*+(?:#[^\n]*+)?$')
SECTION_VALUE = re.compile(r'(?m)^[ \t]*+["\']?[\w.-]++["\']?[ \t]*+[=:][ \t]*+(?P<v>[^\s#][^\n]*+)')
AWS_SET = re.compile(r'\baws[ \t]++configure[ \t]++set[ \t]++(?P<l>[\w.-]++)[ \t]++(?P<v>[^\s\'"`]++)')
AUTH_HEADER = re.compile(
    r'(?i)(?<![\w-])(?:proxy-)?authorization(?:\\*["\'])?[ \t]*+[:=](?![:=])(?:[ \t]|\\t)*+'
    r'(?P<v>' + QUOTED + r'|(?:(?!\\[nr])[^\r\n])++)'
    r'|(?<![\w-])(?:set-)?cookie(?:\\*["\'])?[ \t]*+[:=](?![:=])(?:[ \t]|\\t)*+'
    r'(?P<c>' + QUOTED + r'|(?:(?!\\[nr])[^\r\n])++)')
SCHEME = re.compile(r'(?:\\*["\'])?(?i:bearer|basic|token)[ \t]++([^\s"\'\\,;)]*+)')
BEARER = re.compile(r'(?i)\b(?:bearer|basic)[ \t]++(?P<v>[A-Za-z0-9._~+/-]{8,}+=*+)')
PRIVATE_HEADER = re.compile(r'-----BEGIN (?:[A-Z0-9]++ )*?PRIVATE KEY(?: BLOCK)?-----')
PRIVATE_END = re.compile(r'-----END (?:[A-Z0-9]++ )*?PRIVATE KEY(?: BLOCK)?-----')
# Without an END marker, a key runs while lines look like key material or PEM headers.
KEY_LINES = re.compile(r'(?:(?:\r?\n|\\r?\\n)(?:[ \t]*+(?:[A-Za-z0-9+/=]{1,128}+|[\w-]{1,32}+:'
                       r'[^\r\n\\]{0,200}+)(?=[ \t]|\r?\n|\\[rn]|["\']|$))?)*+')
TOKEN_CHAR = re.compile(r'[A-Za-z0-9+/=_.~:@%-]')
NOT_TOKEN = re.compile(r'[^A-Za-z0-9+/=_.~:@%-]')
# Long tokens with mixed character classes and few readable words look random.
CANDIDATE = re.compile(r'(?<![A-Za-z0-9+/_-])[A-Za-z0-9+/_-]{24,}+=*+')
WORDISH = re.compile(r'[A-Z]?[a-z]{3,}|[A-Z]{4,}(?![a-z])')
SAFE_TOKEN = re.compile(r'(?i)[0-9a-f]{40}|[0-9a-f]{64}|[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}'
                        r'|[0-9a-f]{12}4[0-9a-f]{3}[89ab][0-9a-f]{15}|tr-[0-9a-f]{32}|prismabuild-job[0-9a-f]{32}'
                        r'|[A-Za-z]{1,16}-[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}'
                        r'|sha(?:1|224|256|384|512)-[A-Za-z0-9+/]+=*')
# Names that hold credentials whatever their extension.
SENSITIVE_NAME = re.compile(
    r'(?i)\.env[^/]*|[^/]*\.env|env(?:\.[^/]*)?|[^/]*\.(?:pem|key|ppk|p12|pfx|jks|keystore|p8|pk8)'
    r'|[^/]*\.tfvars(?:\.json)?|[^/]*\.tfstate(?:\.backup)?|\.netrc|_netrc|\.npmrc|\.pypirc|\.pgpass|\.htpasswd'
    r'|\.my\.cnf|\.dockercfg|\.s3cfg|\.boto|\.vault_pass|[^/]*vault[-_.]pass[^/]*|\.git-credentials|\.token'
    r'|token\.json|auth\.json'
    r'|service-?account[^/]*\.json|[^/]*sa-key[^/]*\.json|[^/]*kubeconfig[^/]*|shadow|wp-config\.php')
# Broad names and directories, which also name ordinary source and docs.
SENSITIVE_BROAD = re.compile(r'(?i)(?:^|/)(?:secrets?|credentials?|\.ssh|\.gnupg)/'
                             r'|(?:^|/)(?:id_[^/]*|[^/]*secret[^/]*|[^/]*credentials[^/]*)$')
SOURCE_FILE = re.compile(r'(?i)\.(?:py|pyi|rs|go|js|jsx|ts|tsx|java|kt|scala|c|h|cc|cpp|hpp|rb|cs|swift|sh'
                         r'|md|rst|adoc|html|css|csv|tsv|pub)$')
PATH_KEYS = {'path', 'file', 'file_path', 'filepath', 'filename', 'target_file'}
CONTENT_KEYS = {'content', 'text', 'result', 'output', 'stdout', 'old', 'new', 'data', 'body', 'lines',
                'artifact'}
SKIPPED_KEYS = {'response_metadata', 'additional_kwargs', 'headers'}

# Each pattern redacts group `v` (or `c`), else the whole match. Patterns with a
# check redact only when it accepts the match.
PATTERNS = [
    re.compile(r'Private-Lines:[ \t]*+\d++[ \t]*+\r?\n(?P<v>(?:(?!Private-MAC:)[^\r\n]++\r?\n?)++)'),
    re.compile(r'\beyJ[A-Za-z0-9_-]*+\.[A-Za-z0-9_-]++\.[A-Za-z0-9_-]*+(?:\.[A-Za-z0-9_-]*+){0,2}+'),
    re.compile(r'\b(?:sk-[A-Za-z0-9_-]{20,}+|(?:sk|rk)_(?:live|test)_[A-Za-z0-9]{10,}+|whsec_[A-Za-z0-9+/=]{20,}+'
               r'|gh[pousr]_[A-Za-z0-9_]{20,}+|github_pat_[A-Za-z0-9_]{20,}+'
               r'|gl(?:pat|rt|ptt|dt|cbt|soat|ft|imt|oas)-[A-Za-z0-9_-]{16,}+(?:\.[0-9a-z]{2}\.[A-Za-z0-9]{6,}+)?'
               r'|(?:AKIA|ASIA|AGPA|AIDA|AROA|ANPA)[A-Z0-9]{16}|xox[abeprs]-[A-Za-z0-9-]{10,}+'
               r'|xapp-[A-Za-z0-9-]{10,}+|pypi-[A-Za-z0-9_-]{16,}+|npm_[A-Za-z0-9]{20,}+|hf_[A-Za-z0-9]{20,}+'
               r'|AIza[A-Za-z0-9_-]{20,}+|SG\.[A-Za-z0-9_-]{10,}+\.[A-Za-z0-9_-]{10,}+|ya29\.[A-Za-z0-9_-]{20,}+'
               r'|GOCSPX-[A-Za-z0-9_-]{20,}+|1//0[A-Za-z0-9_-]{20,}+|SK[0-9a-fA-F]{32}|key-[0-9a-f]{32}'
               r'|do[opr]_v1_[a-f0-9]{64}|shp(?:at|ca|pa|ss)_[a-fA-F0-9]{32}|dckr_pat_[A-Za-z0-9_-]{20,}+'
               r'|dapi[a-f0-9]{32}(?:-\d)?|ATATT3[A-Za-z0-9_=-]{20,}+|lin_api_[A-Za-z0-9]{20,}+|sbp_[a-f0-9]{40}'
               r'|PMAK-[a-f0-9]{24}-[a-f0-9]{24,}+|ntn_[A-Za-z0-9]{20,}+|secret_[A-Za-z0-9]{30,}+'
               r'|hv[sbr]\.[A-Za-z0-9_-]{20,}+|pat[A-Za-z0-9]{14}\.[a-f0-9]{64}'
               r'|[A-Za-z0-9]{14}\.atlasv1\.[A-Za-z0-9_-]{60,}+'
               r'|[MNO][A-Za-z0-9_-]{23,25}\.[A-Za-z0-9_-]{6}\.[A-Za-z0-9_-]{27,40})'),
    # Telegram bot tokens also sit inside URLs, as in /bot123456789:AA...
    re.compile(r'(?<!\d)\d{8,10}:AA[A-Za-z0-9_-]{30,}+'),
    # A Netscape cookie jar line ends with the cookie's name and value.
    re.compile(r'(?<!\S)(?:#HttpOnly_)?\S++\t(?:TRUE|FALSE)\t[^\t\n]*+\t(?:TRUE|FALSE)\t\d++\t[^\t\n]++\t'
               r'(?P<v>[^\t\n]++)'),
    # Password hashes from htpasswd, shadow and bcrypt can be cracked offline.
    re.compile(r'\$(?:apr1|[1256]|y|gy|7)\$[^\s:$]{0,64}\$(?P<v>[./A-Za-z0-9]{16,}+)'),
    re.compile(r'\$2[abxy]?\$\d\d\$(?P<v>[./A-Za-z0-9]{53})'),
    re.compile(r'(?i)\bIDENTIFIED[ \t]++(?:WITH[ \t]++\S++[ \t]++)?BY[ \t]++(?:PASSWORD[ \t]*+\(?[ \t]*+)?'
               r'(?P<v>\'[^\'\n]{1,256}\'|"[^"\n]{1,256}")'),
    re.compile(r'(?i)\b(?:hooks\.slack\.com/services|discord(?:app)?\.com/api/webhooks|outlook\.office(?:365)?'
               r'\.com/webhook|(?<![\w-])[\w-]++\.webhook\.office\.com/webhookb2)/[^\s"\'\\]+'),
    re.compile(r'\bsshpass[ \t]++-p[ \t]*+(?P<v>[^\s\'"`]++)'),
    re.compile(r'\b(?:docker|podman|buildah|helm|skopeo)(?:[ \t]++registry)?[ \t]++login\b[^\n]{0,200}?'
               r'[ \t]-p[ \t]++(?P<v>[^\s\'"`]++)'),
    re.compile(r'\b(?:mysql|mysqldump|mysqladmin|mariadb)\b[^\n]{0,200}?[ \t]-p(?P<v>[^\s\'"`]++)'),
    re.compile(r'\bhtpasswd\b[^\n]{0,100}?[ \t]-[a-zA-Z]*b[a-zA-Z]*+(?:[ \t]++-[a-zA-Z]++)*+'
               r'[ \t]++\S++[ \t]++\S++[ \t]++(?P<v>[^\s\'"`]++)'),
    re.compile(r'(?<![\w-])-pass(?:in|out)?[ \t]++pass:(?P<v>[^\s\'"`]++)'),
    re.compile(r'\b(?:unzip|zip)\b[^\n]{0,100}?[ \t]-P[ \t]*+(?P<v>[^\s\'"`]++)'),
    re.compile(r'\b7za?\b[^\n]{0,100}?[ \t]-p(?P<v>[^\s\'"`]++)'),
    re.compile(r'\blogin[ \t]++\S++[ \t]++password[ \t]++(?P<v>\S++)'),
    re.compile(r'(?m)^[ \t]*+password[ \t]++(?P<v>\S++)[ \t]*+$'),
    re.compile(r'(?<!\S)[A-Za-z0-9._*-]++:(?:\d++|\*):[\w.*-]++:[\w.@*-]++:(?P<v>[^\s:]++)(?!\S)'),
]


def label_kind(label):
    """'strong' when a label names a secret, 'weak' when it may, else None."""
    if len(label) > 128:
        return None
    words = [word.lower() for word in LABEL_WORDS.findall(label)]
    if not words or NOT_SECRET.intersection(words):
        return None
    for previous, word in zip([''] + words, words):
        if (word in SECRET_WORDS or word.endswith('token') or any(part in word for part in SECRET_PARTS)
                or (word == 'key' and previous in KEY_QUALIFIERS)
                or (previous, word) in {('session', 'id'), ('key', 'value')}):
            return 'strong'
    return 'weak' if WEAK_WORDS.intersection(words) else None


def unquote(value):
    stripped = re.sub(r'^[rbfuRBFU]{1,2}(?=["\'])', '', value).lstrip('\\')
    if stripped[:1] in ('"', "'", '`') and len(stripped) >= 2:
        return stripped[1:-1].rstrip('\\'), True
    return value, False


def placeholder(value):
    """Words only, one of which says the value is not real, as in fake-token."""
    return (re.fullmatch(r'[A-Za-z]+(?:[-_ .:][A-Za-z]+)*', value) is not None
            and bool(PLACEHOLDERS.intersection(word.lower() for word in LABEL_WORDS.findall(value))))


def plausible(kind, label, value, after='', assign='', code=False):
    """Whether a labelled value can be a secret rather than code, a type, prose or a fixture."""
    inner, quoted = unquote(value)
    if (not kind or not inner.strip() or REFERENCE.match(inner) or PATHLIKE.fullmatch(inner)
            or TEMPLATE.search(inner) or VERSION.fullmatch(inner) or inner.lower() == label.lower()
            or SCHEME_NAME.fullmatch(inner.strip()) or placeholder(inner[:64])):
        return False
    if quoted and (after.lstrip()[:1] in ('*', '+') or re.match(r'\s*\+|.*\+\s*$', inner)):
        return False  # 'b' * 64, or the end of one string literal and the start of the next
    if not quoted:
        if (inner in KEYWORDS or inner in TYPE_NAMES or any(c in inner for c in '{([') or inner[0] in '&-'
                or (after and after[0] in '([') or DOTTED.fullmatch(inner) or NUMBER.fullmatch(inner)):
            return False
        if assign == ':' and re.fullmatch(r'[A-Z]\w*', inner) and re.match(r'[ \t]*[<\[,)|=]', after):
            return False
        if IDENTIFIER.fullmatch(inner) and label_kind(inner):
            return False
        if CODE_NAME.fullmatch(inner) and code:
            return False
        if WORD.fullmatch(inner) and re.match(r'[ \t]+[^\s#]', after):
            return False
        if re.fullmatch(r'[A-Za-z]{2,12}\d{0,3}', inner) and re.match(r'[ \t]+[a-z]{2,}', after):
            return False
    if kind == 'weak':
        # Mixed upper case, lower case and digits in one run, as in DB_PASS=CVwg8aolbgRxKBOL.
        sample = inner[:64]
        return random_like(sample, 12) or (
            re.fullmatch(r'[A-Za-z0-9]{12,}', sample) is not None and any(map(str.isupper, sample))
            and any(map(str.islower, sample)) and any(map(str.isdigit, sample)))
    return True


def token_shaped(value):
    return not WORD.fullmatch(value) and (any(map(str.isdigit, value)) or any(c in '._~+/=-' for c in value)
                                          or any(map(str.isupper, value[1:])))


def random_like(token, minimum=20):
    """Frequent switches between upper case, lower case and digits, or few readable words."""
    if SAFE_TOKEN.fullmatch(token) or any(map(str.isspace, token)):
        return False
    parts = re.split(r'[-_]', token)
    if len(parts) > 1 and all(re.fullmatch(r'[a-z0-9]*', part) or len(part) <= 2 for part in parts):
        # A kebab or snake name such as tl3-sandbox-dl380g10-slot-1 is judged by its parts.
        return any(len(part) >= max(minimum, 24) and random_like(part, minimum) for part in parts)
    classes = [2 if c.isdigit() else c.isupper() for c in token if c.isalnum()]
    if len(classes) < minimum or len(set(classes)) < 2:
        return False
    switches = sum(a != b for a, b in zip(classes, classes[1:]))
    words = sum(map(len, WORDISH.findall(token)))
    # Readable words carry vowels; random letters rarely do. FileNotFoundError switches
    # case often but reads as words.
    letters = [c for c in token if c.isalpha()]
    vowels = sum(c in 'aeiouAEIOU' for c in letters)
    return ((5 * switches >= 2 * len(classes) and (10 * words < 7 * len(classes) or 4 * vowels < len(letters)))
            or (2 * words < len(classes) and 5 * switches >= len(classes)))


def sensitive_file(path):
    """Whether a path names a file that holds credentials, judged by its components."""
    if not isinstance(path, str) or len(path) > 4096:
        return False
    path = path.replace('\\', '/').strip().rstrip('/')
    name = path.rsplit('/', 1)[-1].strip()
    if path.endswith(('.docker/config.json', '.kube/config')):
        return True
    if SENSITIVE_NAME.fullmatch(name):
        # env.py and id_map.csv are code and data, but .env.sh is still an env file.
        return name.startswith('.env') or not SOURCE_FILE.search(name)
    return SENSITIVE_BROAD.search(path) is not None and not SOURCE_FILE.search(name)


def file_marker(path):
    return f'[omitted: content of sensitive file {path}]'


def sensitive_path(value):
    """The first sensitive file named by a path argument in a bounded walk of value."""
    stack, seen = [value], 0
    while stack and seen < 10000:
        node, seen = stack.pop(), seen + 1
        if isinstance(node, dict):
            for key, item in node.items():
                if key in PATH_KEYS and sensitive_file(item):
                    return item
                stack.append(item)
        elif isinstance(node, list):
            stack.extend(node)
    return None


def check_labelled(match, text):
    label, value = match['l'], match['v']
    if label_kind(label) == 'strong' and label.lower() in {'signature', 'sig'} and readable(value):
        return None
    gap = text[match.end('a'):match.start('v')]
    if '\n' in gap:
        # A value on the next line belongs to the label only when indented under it.
        line = text.rfind('\n', 0, match.start()) + 1
        indent = len(text[line:match.start()]) - len(text[line:match.start()].lstrip(' \t'))
        if match.start('v') - (text.rfind('\n', 0, match.start('v')) + 1) <= indent:
            return None
    after = text[match.end('v'):match.end('v') + 8]
    # Bounded scans keep a long line of labels linear.
    line_start = text.rfind('\n', max(0, match.start() - 80), match.start()) + 1
    line_start = max(line_start, match.start() - 80)
    if WORD.fullmatch(value) and len(re.findall(r'[A-Za-z]{2,}[ \t]', text[line_start:match.start()])) >= 2:
        return None  # "with the same attempt token: reports"
    # A quoted key, or spaces around =, mark code, where a bare name is a variable.
    code = (bool(match['q']) and match['q'][-1] in '"\'`') or (
        match['a'] == '=' and (text[match.start('a') - 1:match.start('a')].isspace() or gap[:1].isspace()))
    kind = label_kind(label)
    return match.span('v') if plausible(kind, label, value, after, match['a'], code) else None


def check_flag(match, text):
    return match.span('v') if plausible(label_kind(match['l']), match['l'], match['v'], '', '=') else None


def check_table(match, text):
    label, value = match['l'].strip('`* '), match['v'].strip('`')
    if label_kind(label) and not REFERENCE.match(value) and len(value) >= 8 and token_shaped(value) \
            and not any(map(str.isspace, value)):
        return match.span('v')
    return None


def check_block(match, text):
    return match.span('v') if label_kind(match['l']) == 'strong' else None


def check_name_value(match, text):
    label = match['l']
    return match.span('v') if plausible(label_kind(label), label, match['v']) else None


def readable(value):
    """Words and separators only, such as an incident class like GitHub:permission."""
    inner = unquote(value)[0]
    return re.fullmatch(r'[A-Za-z][A-Za-z:/._ -]*', inner) is not None and not random_like(inner, 8)


def check_header(match, text):
    group = 'v' if match['v'] is not None else 'c'
    value = match[group]
    inner = unquote(value)[0]
    if (REFERENCE.match(inner) or TEMPLATE.search(inner) or placeholder(inner)
            or re.match(r'[A-Za-z_][\w.]*+[ \t]*+(?:[,)}]|$)', value)):
        return None  # a variable or attribute, as in "Authorization": self.authorization,
    if scheme := SCHEME.match(value):
        token = scheme[1]
        if not token or REFERENCE.match(token) or TEMPLATE.search(token) or placeholder(token):
            return None
        return match.start(group), match.start(group) + scheme.end(1)
    return match.span(group)


def check_bearer(match, text):
    value = match['v']
    return match.span('v') if token_shaped(value) and not placeholder(value) else None


def check_url(match, text):
    value = match['v']
    if REFERENCE.match(value) or TEMPLATE.search(value) or placeholder(value):
        return None
    return match.span('v')


def check_user(match, text):
    return None if match['v'].isdigit() or TEMPLATE.search(match['v']) else match.span('v')


def check_aws(match, text):
    return match.span('v') if label_kind(match['l']) else None


def secretish(value, quoted=False):
    """An unlabelled token near a secret word that looks like a password, not a word or a number."""
    core = (value if quoted else value.rstrip('.,:;)')).strip('*_')
    if (len(core) < 8 or any(map(str.isspace, core)) or REFERENCE.match(core) or TEMPLATE.search(core)
            or SAFE_TOKEN.fullmatch(core) or PATHLIKE.fullmatch(core) or '://' in core or DOTTED.fullmatch(core)
            or re.fullmatch(r'[\d.:,/+-]+', core) or placeholder(core)):
        return False
    if random_like(core, 12):
        return True
    if '/' in core:
        return False  # a path or a reference such as owner/repo#12
    upper, lower, digit = (any(map(str.isupper, core)), any(map(str.islower, core)),
                           any(map(str.isdigit, core)))
    special = sum(c in '!#%^*~@$&+?' for c in core)
    return (upper and lower and digit) or (special >= 2 and (upper or digit)) or (
        quoted and digit and (upper or lower) and special >= 1)


def check_ssh_keygen(match, text):
    return match.span('v') if unquote(match['v'])[0] else None


def check_xml(match, text):
    label, value = match['l'], match['v']
    if len(value.strip()) >= 4 and label_kind(label) == 'strong' and plausible('strong', label, value, assign='='):
        return match.span('v')
    return None


def check_json_pair(match, text):
    label, value = match['l'], match['v']
    if len(unquote(value)[0]) < 4:
        return None
    return match.span('v') if plausible(label_kind(label), label, value, assign=':') else None


def check_pair(match, text):
    return match.span('v') if secretish(match['v']) else None


CHECKED = [(LABELLED, check_labelled), (FLAG, check_flag), (TABLE, check_table), (BLOCK_SCALAR, check_block),
           (NAME_VALUE, check_name_value), (AUTH_HEADER, check_header), (BEARER, check_bearer),
           (AWS_SET, check_aws),
           # The password is everything up to the last @ before the host, so it may contain @ or /.
           (re.compile(r'(?i)(?<![\w/])//[^\s/:@]{0,128}:(?!\d++(?:[/?#\s]|$))(?P<v>\S{1,256})@'
                       r'(?=[^\s@]{0,253}(?:[\s/?#:"\']|$))'), check_url),
           (re.compile(r'(?<![\w/])//(?P<v>[^\s/:@]{20,128})@'), check_url),
           (re.compile(r'(?<![\w-])(?:--(?:proxy-)?user|-[uU])(?:=|[ \t]++)[^\s:\'"]++:(?P<v>[^\s\'"`]++)'),
            check_user),
           (re.compile(r'\bssh-keygen\b[^\n]{0,100}?[ \t]-N[ \t]++(?P<v>' + QUOTED + r'|[^\s\'"]++)'),
            check_ssh_keygen),
           # <password>x</password>, and <property name="db.password">x</property>.
           (re.compile(r'<(?P<l>[A-Za-z_][\w.:-]{0,63})(?:[ \t][^<>]{0,256})?>(?P<v>[^<>\n]{1,512})'
                       r'</(?P=l)[ \t]*>'), check_xml),
           (re.compile(r'<(?P<t>[A-Za-z_][\w.:-]{0,63})[ \t][^<>]{0,256}?\b(?:name|key)[ \t]*=[ \t]*["\']'
                       r'(?P<l>[^"\'<>]{1,128})["\'][^<>]{0,256}>(?P<v>[^<>\n]{1,512})</(?P=t)[ \t]*>'),
            check_xml),
           # {"name": "DB_PASSWORD", "value": "x"} in JSON text, however escaped or laid out.
           (re.compile(r'(?<!\\)(?P<q>\\*")(?:name|key|Name|Key)(?P=q)' + SPACE + ':' + SPACE + r'(?P=q)'
                       r'(?P<l>[\w.-]{1,128})(?P=q)' + SPACE + ',' + SPACE + r'(?P=q)(?:value|Value)(?P=q)' + SPACE
                       + ':' + SPACE + r'(?P<v>' + QUOTED + r'|[^\s,}\]]{1,512})'), check_json_pair),
           # Login: admin / password
           (re.compile(r'(?i)\b(?:login|credentials?|creds|user(?:name)?)[ \t]*+:[ \t]*+[^\s/]{1,64}'
                       r'[ \t]*+/[ \t]*+(?P<v>[^\s/]{1,256})'), check_pair)]


# A secret word, then within NEAR_CHARS a quoted or password-like token:
# "the password for staging is X", WITH PASSWORD 'X', AUTH X, net use ... X.
NEAR_WORD = re.compile(r'(?i)(?<![\w-])(?:password|passwd|passphrase|pw|secret|token|api[-_ ]?key|credentials?'
                       r'|requirepass|masterauth|convertto-securestring)(?![\w-])|(?<![\w-])(?-i:AUTH)(?=[ \t])'
                       r'|(?i:\bnet[ \t]++use)\b')
NEAR_TOKEN = re.compile(r'(?P<q>["\'`])(?P<qv>[^"\'`\s]{1,256})(?P=q)|(?P<v>[^\s"\'`<>()\[\]{},;|]{1,256})')
NEAR_CHARS = 48
# The data of a Kubernetes Secret manifest, where every value is a base64 secret.
SECRET_DATA = re.compile(r'(?m)^kind:[ \t]*+Secret[ \t]*+$(?:\n(?!kind:)[^\n]*+){0,40}?\n(?P<i>[ \t]*+)'
                         r'(?:data|stringData):[ \t]*+\r?\n(?P<b>(?:(?P=i)[ \t]++[^\n]*+(?:\n|\Z))++)')


def near_spans(text):
    found = []
    for word in NEAR_WORD.finditer(text):
        line_end = text.find('\n', word.end(), word.end() + NEAR_CHARS + 256)
        line_end = min(len(text), word.end() + NEAR_CHARS + 256) if line_end == -1 else line_end
        limit = min(word.end() + NEAR_CHARS, line_end)
        for token in NEAR_TOKEN.finditer(text, word.end(), line_end):
            if token.start() >= limit:
                break
            if token['qv'] is not None and secretish(token['qv'], quoted=True):
                found.append(token.span('qv'))
            elif token['v'] is not None and secretish(token['v']):
                found.append(token.span('v'))
    return found


def csv_spans(text):
    """Cells under a CSV header cell that names a secret, such as username,password,role."""
    found, columns, width, position = [], None, 0, 0
    for line in text.split('\n') if ',' in text else ():
        cells = line.split(',')
        if columns and len(cells) == width:
            offset = position
            for index, cell in enumerate(cells):
                if index in columns and cell.strip() and label_kind(cell.strip()) != 'strong':
                    found.append((offset, offset + len(cell)))
                offset += len(cell) + 1
        elif 2 <= len(cells) <= 64 and all(re.fullmatch(r'[\w .-]{1,40}', cell) for cell in cells):
            columns = {index for index, cell in enumerate(cells) if label_kind(cell.strip()) == 'strong'}
            width = len(cells)
        else:
            columns = None
        position += len(line) + 1
    return found


class Clip:
    """The shown windows of a long string. Each source keeps CONTEXT characters
    beyond its window so a later cut can still redact across it."""

    def __init__(self, parts, lead=False, gap=0):
        self.parts = parts  # [(source, start, end)], one window or a head and a tail
        self.lead = lead    # text before the first window was omitted
        self.gap = gap      # characters omitted between head and tail

    def __len__(self):
        return sum(end - start for _, start, end in self.parts)

    def shown(self):
        """The text before redaction, with the notes the rendered text carries."""
        middle = f' [truncated: {self.gap} characters omitted] '
        return (LEAD_NOTE if self.lead else '') + middle.join(
            source[start:end] for source, start, end in self.parts)

    def compact(self):
        parts = []
        for source, start, end in self.parts:
            low, high = max(0, start - CONTEXT), min(len(source), end + CONTEXT)
            parts.append((source[low:high], start - low, end - low))
        return Clip(parts, self.lead, self.gap)


class Json:
    """A string that held JSON: its shaped value, and the raw text while short."""

    def __init__(self, value, raw):
        self.value, self.raw = value, raw


def settle(text, pos, end):
    """Move a cut forward so it never starts inside a private key block or a token."""
    begin = text.rfind('-----BEGIN ', 0, pos)
    if begin != -1 and PRIVATE_HEADER.match(text, begin) and text.find('-----END ', begin, pos) == -1:
        close = text.find('-----END ', pos, end)
        close = text.find('-----', close + 9, end) if close != -1 else -1
        pos = close + 5 if close != -1 else end
    if 0 < pos < end and TOKEN_CHAR.match(text, pos - 1) and TOKEN_CHAR.match(text, pos):
        after = NOT_TOKEN.search(text, pos, end)
        pos = after.start() if after else end
    return pos


def tail_window(source, start, end, limit):
    """Newest complete lines when possible, else the newest characters."""
    cut = source.find('\n', max(start, end - limit), end)
    if cut != -1 and end - cut > limit // 2:
        return settle(source, cut + 1, end), True
    return settle(source, max(start, end - limit), end), False


def clip(value, limit):
    """Bound a string or Clip to about limit shown characters."""
    if isinstance(value, Clip):
        parts, lead, gap = value.parts, value.lead, value.gap
    else:
        parts, lead, gap = [(value, 0, len(value))], False, 0
    if sum(end - start for _, start, end in parts) <= limit:
        return Clip(parts, lead, gap)
    head = limit // 4
    if len(parts) == 1:
        source, start, end = parts[0]
        begin, whole_lines = tail_window(source, start, end, limit)
        if whole_lines:
            return Clip([(source, begin, end)], True)
        return Clip([(source, start, start + head), (source, begin, end)], lead, begin - start - head)
    (head_source, head_start, head_end), (source, start, end) = parts
    new_end = min(head_end, head_start + head)
    begin = max(start, tail_window(source, start, end, limit - head)[0])
    return Clip([(head_source, head_start, new_end), (source, begin, end)], lead,
                gap + (head_end - new_end) + (begin - start))


def size(value):
    return len(json.dumps(value, ensure_ascii=False,
                          default=lambda item: item.shown() if isinstance(item, Clip) else item.value).encode())


class Redactor:
    def __init__(self, key_path):
        # The runtime supplies an already-loaded key. Tracing never opens key files.
        self.secrets = {key_path} if key_path else set()
        self.calls = {}  # tool call id -> sensitive file it read or wrote
        self.truncated = False

    def spans(self, text):
        found = []
        for secret in self.secrets:
            if secret:
                start = text.find(secret)
                while start != -1:
                    found.append((start, start + len(secret)))
                    start = text.find(secret, start + 1)
        ends = [match.end() for match in PRIVATE_END.finditer(text)]
        reach = 0
        for begin in PRIVATE_HEADER.finditer(text):
            if begin.start() < reach:
                continue
            end = next((end for end in ends if end > begin.end()), None)
            reach = end if end is not None else KEY_LINES.match(text, begin.end()).end()
            found.append((begin.start(), reach))
        for pattern in PATTERNS:
            group = 'v' if 'v' in pattern.groupindex else 0
            found.extend(match.span(group) for match in pattern.finditer(text))
        for pattern, check in CHECKED:
            found.extend(span for match in pattern.finditer(text) if (span := check(match, text)))
        found.extend(near_spans(text))
        if 'Secret' in text:
            for block in SECRET_DATA.finditer(text):
                values = SECTION_VALUE.finditer(text, block.start('b'), block.end('b'))
                found.extend(match.span('v') for match in values)
        found.extend(csv_spans(text))
        sections = list(SECTION.finditer(text))
        for index, section in enumerate(sections):
            if label_kind(section['l'].strip()) == 'strong':
                stop = sections[index + 1].start() if index + 1 < len(sections) else len(text)
                found.extend(match.span('v') for match in SECTION_VALUE.finditer(text, section.end(), stop))
        for match in CANDIDATE.finditer(text):
            token, position = match.group(), match.start()
            if text.startswith(('...', '\u2026'), match.end()) and re.fullmatch(r'[0-9a-f]+', token):
                continue
            segments = token.split('/')
            if len(segments) > 1 and any(PATH_SEGMENT.fullmatch(segment) for segment in segments):
                # A path: judge its segments, so hex-encoded or digest segments don't
                # condemn the readable path around them.
                for segment in segments:
                    if len(segment) >= 24 and random_like(segment):
                        found.append((position, position + len(segment)))
                    position += len(segment) + 1
                continue
            if random_like(token):
                found.append(match.span())
                continue
            if SAFE_TOKEN.fullmatch(token):
                continue
            for piece in re.split(r'[/_.-]', token):
                if len(piece) >= 24 and random_like(piece):
                    found.append((position, position + len(piece)))
                position += len(piece) + 1
        merged = []
        for start, end in sorted(found):
            if end <= start:
                continue
            if merged and start <= merged[-1][1]:
                merged[-1][1] = max(merged[-1][1], end)
            else:
                merged.append([start, end])
        return merged

    def window(self, source, start, end):
        """Redact source[start:end], seeing CONTEXT characters on each side."""
        low, high = max(0, start - CONTEXT), min(len(source), end + CONTEXT)
        text, position, stop = source[low:high], start - low, end - low
        out = []
        for first, last in self.spans(text):
            if last <= position or first >= stop:
                continue
            if first > position:
                out.append(text[position:first])
            out.append(REDACTED)
            position = last
        if position < stop:
            out.append(text[position:stop])
        return ''.join(out)

    def text(self, value, limit=STRING_CHARS):
        shown = clip(value, limit)
        if shown.lead or len(shown.parts) > 1:
            self.truncated = True
        pieces = [LEAD_NOTE] if shown.lead else []
        for index, (source, start, end) in enumerate(shown.parts):
            if index:
                pieces.append(f' [truncated: {shown.gap} characters omitted] ')
            pieces.append(self.window(source, start, end))
        return ''.join(pieces)

    def note_calls(self, value):
        """Remember tool calls that read or wrote a sensitive file."""
        stack, seen = [value], 0
        while stack and seen < 200000:
            node, seen = stack.pop(), seen + 1
            if isinstance(node, dict):
                call, args = node.get('id'), node.get('args', node.get('arguments'))
                if isinstance(node.get('function'), dict):
                    args = node['function'].get('arguments', args)
                if isinstance(args, str) and args.lstrip()[:1] == '{':
                    try:
                        args = json.loads(args)
                    except (ValueError, RecursionError):
                        pass
                if isinstance(call, str) and (path := sensitive_path(args)):
                    self.calls[call] = path
                stack.extend(node.values())
            elif isinstance(node, list):
                stack.extend(node)

    def entries(self, value):
        """A dict's fields, newest first, with field-level redaction. Fields are
        judged as the walk reaches them, so a huge dict costs only its budget."""
        path = next((value[key] for key in PATH_KEYS if sensitive_file(value.get(key))), None)
        call = value.get('tool_call_id')
        path = path or (self.calls.get(call) if isinstance(call, str) else None)
        named = next((value[key] for key in ('name', 'key', 'Name', 'Key') if isinstance(value.get(key), str)), '')
        for key in reversed(value):
            if key in SKIPPED_KEYS:
                continue
            item, key_text = value[key], str(key)
            kind = 'strong' if len(key_text) > 128 else label_kind(key_text)
            if path and key in CONTENT_KEYS:
                item = file_marker(path)
            elif kind == 'strong' and item is not None and not isinstance(item, bool):
                item = REDACTED
            elif kind == 'weak' and isinstance(item, str) and plausible('weak', key_text, item):
                item = REDACTED
            elif key in ('value', 'Value') and label_kind(named) == 'strong' and isinstance(item, (str, int)):
                item = REDACTED
            yield key_text[:128], item

    def walk(self, value, limit, depth, leaf):
        if limit < 128 or depth > 20:
            self.truncated = True
            return '[truncated: content budget]'
        if isinstance(value, (str, Clip, Json)):
            return leaf(value, limit, depth)
        if leaf == self.clean_leaf and isinstance(value, (dict, list)) and size(value) <= limit - 32:
            # Content that already fits passes whole, without the reserves that
            # budget a cut, unless redaction made it grow past the limit.
            result = self.container(value, limit, depth, leaf, True)
            if size(result) <= limit:
                return result
        return self.container(value, limit, depth, leaf, False)

    def container(self, value, limit, depth, leaf, roomy):
        if isinstance(value, dict):
            result, remaining = {}, limit - 96
            for index, (key, item) in enumerate(self.entries(value)):
                if remaining < 160 and not roomy:
                    result['tl3.truncated'] = 'earlier fields omitted'
                    self.truncated = True
                    break
                key = key if leaf == self.shape_leaf else self.text(clip(key, 128))
                share = remaining - size(key) - 8
                if roomy:
                    share = size(item) + 512
                else:
                    share = min(share, max(ITEM_BYTES, share // max(1, len(value) - index)))
                item = self.walk(item, share, depth + 1, leaf)
                remaining -= size({key: item}) + 2
                result[key] = item
            return dict(reversed(list(result.items())))
        if isinstance(value, list):
            result, remaining = [], limit - 96
            for index, item in enumerate(reversed(value)):
                if remaining < 160 and not roomy:
                    result.append({'truncated': True, 'note': 'earlier items omitted'})
                    self.truncated = True
                    break
                if roomy:
                    share = size(item) + 512
                else:
                    share = min(remaining, max(ITEM_BYTES, remaining // (len(value) - index)))
                item = self.walk(item, share, depth + 1, leaf)
                remaining -= size(item) + 2
                result.append(item)
            return list(reversed(result))
        return value

    def shape_leaf(self, value, limit, depth):
        if isinstance(value, Json):
            return Json(self.walk(value.value, limit // 2, depth + 1, self.shape_leaf), value.raw)
        if isinstance(value, str) and value[:128].lstrip().startswith(('{', '[', '"')):
            # Model replies and tool arguments often hold JSON on one physical
            # line. Judge it by field names, however deeply it is quoted.
            try:
                decoded = json.loads(value)
            except (ValueError, RecursionError):
                pass
            else:
                # Short JSON that shaping left intact keeps its original layout.
                shaped = self.walk(decoded, limit // 2, depth + 1, self.shape_leaf)
                return Json(shaped, value if len(value) <= STRING_CHARS and shaped == decoded else None)
        shown = clip(value, min(STRING_CHARS, limit - 64))
        if len(shown.parts) == 1 and not shown.lead and not isinstance(value, Clip):
            return value
        return shown.compact()

    def clean_leaf(self, value, limit, depth):
        if isinstance(value, Json):
            cleaned = self.walk(value.value, limit // 2, depth + 1, self.clean_leaf)
            if value.raw is not None and cleaned == value.value and len(value.raw) <= limit // 6 - 64:
                return self.clean_leaf(value.raw, limit, depth)
            return json.dumps(cleaned, ensure_ascii=False)
        chars = min(STRING_CHARS, limit - 64)
        for _ in range(4):
            if chars < 32:
                break
            out = self.text(clip(value, chars))
            used = size(out)
            if used <= limit - 16:
                return out
            chars = int(chars * (limit - 64) / used)
        self.truncated = True
        return '[truncated: content budget]'

    def shape(self, value, limit=PAYLOAD_BYTES):
        """Bound value without redacting it, keeping context for the final cut."""
        self.note_calls(value)
        return self.walk(value, limit, 0, self.shape_leaf)

    def clean(self, value, limit=PAYLOAD_BYTES):
        """Redact shaped content within limit bytes of JSON."""
        return self.walk(value, limit, 0, self.clean_leaf)

    def payload(self, value, limit=PAYLOAD_BYTES):
        return self.clean(self.shape(value), min(PAYLOAD_BYTES, limit))


def record_model_key(key):
    if capture := active.get():
        capture.redactor.secrets.add(key)


def chunk_counts(usage):
    if not isinstance(usage, dict):
        return {}
    counts = {key: value for key in ('prompt_tokens', 'completion_tokens', 'total_tokens',
                                     'cache_write_tokens')
              if type(value := usage.get(key)) is int and value >= 0}
    for key, fields in [('prompt_tokens_details', ('cached_tokens', 'cache_write_tokens')),
                        ('completion_tokens_details', ('reasoning_tokens',))]:
        if isinstance(details := usage.get(key), dict):
            counts[key] = {field: value for field in fields
                           if type(value := details.get(field)) is int and value >= 0}
    return counts


def record_chunk_usage(chunk):
    try:
        if active.get() is None or not chunk.get('usage'):
            return
        import mlflow
        span = mlflow.get_current_active_span()
        if span is None or span.span_type not in {'LLM', 'CHAT_MODEL'}:
            return
        counts = chunk_counts(chunk['usage'])
        chunks = span.get_attribute('tl3.proxy.usage_chunks') or []
        if len(chunks) < 32:
            span.set_attribute('tl3.proxy.usage_chunks', [*chunks, counts])
        else:
            span.set_attribute('tl3.proxy.usage_chunks_truncated', True)
    except Exception:
        failure()


def record_reply(reply):
    if capture := active.get():
        try:
            capture.root.set_outputs(reply)
        except Exception:
            failure()


def failure():
    print('tl3_workers: MLflow export failed', file=sys.stderr)


class Content:
    __slots__ = ('key', 'value', 'size', 'limit')

    def __init__(self, key, value):
        self.key, self.value, self.size, self.limit = key, value, size(value), 0


class Entry:
    """One ended span: metadata now, and content shaped for redaction at the end."""
    __slots__ = ('proto', 'name', 'kind', 'contents', 'dropped')

    def __init__(self, proto, name, kind, contents):
        self.proto, self.name, self.kind, self.contents, self.dropped = proto, name, kind, contents, False


class Capture:
    def __init__(self, request):
        import mlflow
        import mlflow.langchain
        from mlflow.entities import MlflowExperimentLocation
        from mlflow.tracing import provider
        from mlflow.tracing.processor.mlflow_v3 import MlflowV3SpanProcessor
        from opentelemetry.sdk.trace import TracerProvider
        from opentelemetry.sdk.trace.export import SpanExporter, SpanExportResult

        self.redactor = Redactor(request.get('key_file'))
        self.role = request['role']
        self.model = request['model']['name']
        self.first, self.last = [], deque()
        self.unshrunk = deque()  # entries at full shaped size, oldest first
        self.omitted = 0
        self.shaped = 0
        self.lock = threading.Lock()
        capture = self
        logger = logging.getLogger('mlflow')
        logger.handlers = [logging.NullHandler()]
        logger.propagate = False

        class Exporter(SpanExporter):
            def export(self, spans):
                with capture.lock:
                    for span in spans:
                        try:
                            capture.add(span_entry(span, capture.redactor, capture.role, capture.model))
                        except Exception:
                            # Children of a lost span are attached to the root at export.
                            failure()
                return SpanExportResult.SUCCESS

        # MLflow 3.17.2 has no public in-memory destination. Pin this adapter to
        # that release: the stock exporter starts network threads and joins at exit.
        tracer = TracerProvider(shutdown_on_exit=False)
        tracer.add_span_processor(MlflowV3SpanProcessor(Exporter(), export_metrics=False))
        provider._MLFLOW_TRACE_USER_DESTINATION.set(MlflowExperimentLocation(experiment_id='0'))
        provider.provider.once.do_once(lambda: provider.provider.set(tracer))
        provider.provider.set(tracer)
        mlflow.langchain.autolog(silent=True)
        self.context = mlflow.start_span(name='tl3_' + self.role, span_type='AGENT')
        self.root = self.context.__enter__()
        self.root.set_inputs(request['inputs'])
        trace = request.get('trace', {})
        self.tags = {key: str(trace.get(key, 'unknown'))
                     for key in ('repo', 'issue', 'request', 'attempt', 'round')}
        self.tags.update(node=str(request['operation']['node']),
                         generation=str(request['operation']['generation']),
                         role=request['role'], model=request['model']['name'],
                         effort=request['model']['effort'] or 'default')
        self.settings = request['mlflow']

    def add(self, entry):
        if len(self.first) < FIRST_SPANS:
            self.first.append(entry)
        else:
            self.last.append(entry)
            if len(self.last) > LAST_SPANS:
                dropped = self.last.popleft()
                dropped.dropped = True
                self.omitted += 1
                self.shaped -= sum(content.size for content in dropped.contents)
        self.shaped += sum(content.size for content in entry.contents)
        self.unshrunk.append(entry)
        if len(self.unshrunk) > 2 * (FIRST_SPANS + LAST_SPANS):
            self.unshrunk = deque(older for older in self.unshrunk if not older.dropped)
        if self.shaped > SHAPED_BYTES:
            # Older spans could only receive their floor from the final budget.
            while self.shaped > SHAPED_BYTES // 2 and len(self.unshrunk) > 1:
                older = self.unshrunk.popleft()
                if older.dropped:
                    continue
                for content in older.contents:
                    self.shaped -= content.size
                    content.value = self.redactor.walk(content.value, FLOOR_BYTES, 0,
                                                       self.redactor.shape_leaf)
                    content.size = size(content.value)
                    self.shaped += content.size

    def finish(self, outcome):
        self.context.__exit__(None, None, None)
        from mlflow.tracing.trace_manager import InMemoryTraceManager
        from mlflow.tracing.utils.otlp import _set_otel_proto_anyvalue
        import mlflow.langchain

        InMemoryTraceManager.get_instance().pop_trace(self.root._span.context.trace_id)
        mlflow.langchain.autolog(disable=True, silent=True)
        with self.lock:
            entries = [*self.first, *self.last]
        root = next(entry for entry in entries if not entry.proto.parent_span_id)
        kept = {entry.proto.span_id for entry in entries}
        for entry in entries:
            if entry.proto.parent_span_id and entry.proto.parent_span_id not in kept:
                entry.proto.parent_span_id = root.proto.span_id
            entry.proto.name = self.redactor.text(entry.name)[:256]
        usage = {'input_tokens': 0, 'output_tokens': 0, 'total_tokens': 0}
        for entry in entries:
            for attr in entry.proto.attributes:
                if attr.key == 'mlflow.chat.tokenUsage':
                    for item in attr.value.kvlist_value.values:
                        usage[item.key] = usage.get(item.key, 0) + item.value.int_value
        self.tags.update(outcome=outcome, **{key: str(value) for key, value in usage.items()})
        root.proto.status.code = 1 if outcome == 'succeeded' else 2
        for key, value in self.tags.items():
            root.proto.attributes.add(key='mlflow.traceTag.' + key).value.string_value = \
                self.redactor.text(value)[:512]
        if self.omitted:
            root.proto.attributes.add(key='tl3.trace.omitted_spans').value.int_value = self.omitted
        truncated = self.allocate(root, entries)
        for entry in entries:
            for content in entry.contents:
                value = (self.redactor.clean(content.value, content.limit) if content.limit
                         else {'truncated': True, 'note': 'trace content budget exhausted'})
                _set_otel_proto_anyvalue(entry.proto.attributes.add(key=content.key).value, value)
        if truncated or self.redactor.truncated or self.omitted:
            root.proto.attributes.add(key='tl3.trace.truncated').value.bool_value = True
        from opentelemetry.proto.collector.trace.v1.trace_service_pb2 import ExportTraceServiceRequest
        packet = ExportTraceServiceRequest()
        spans = packet.resource_spans.add().scope_spans.add().spans
        spans.extend(entry.proto for entry in entries)
        # Encoding differs from the JSON sizes the budget uses. Drop the oldest
        # child content until the packet fits.
        for index, entry in enumerate(entries):
            if packet.ByteSize() <= TRACE_BYTES:
                break
            if entry is root:
                continue
            for attr in list(spans[index].attributes):
                if attr.key in {'mlflow.spanInputs', 'mlflow.spanOutputs'}:
                    spans[index].attributes.remove(attr)
        if packet.ByteSize() > TRACE_BYTES:
            failure()
            return
        # An unlinked file bounds handoff and avoids pipe backpressure. The export
        # child owns network waits; the worker never waits for it, even at shutdown.
        with tempfile.TemporaryFile() as payload:
            payload.write(json.dumps(self.settings).encode() + b'\n')
            payload.write(packet.SerializeToString())
            payload.seek(0)
            subprocess.Popen([sys.executable, str(Path(__file__))], stdin=payload,
                             stdout=subprocess.DEVNULL, stderr=sys.stderr, close_fds=True)

    def allocate(self, root, entries):
        """Give the root its content, then the newest model and tool spans, then
        chain spans, which repeat the graph state. Every child keeps a floor."""
        budget = TRACE_BYTES - sum(entry.proto.ByteSize() + 16 for entry in entries) - 32768
        for content in root.contents:
            content.limit = min(PAYLOAD_BYTES, content.size + 256)
            budget -= content.limit
        children = [entry for entry in reversed(entries) if entry is not root]
        order = ([content for entry in children if entry.kind != 'CHAIN' for content in entry.contents]
                 + [content for entry in children if entry.kind == 'CHAIN' for content in entry.contents])
        floor = min(FLOOR_BYTES, max(0, budget // 2) // max(1, len(order)))
        pool = budget - floor * len(order)
        truncated = False
        for content in order:
            need = content.size + 256
            if need <= floor:
                content.limit, pool = need, pool + floor - need
                continue
            extra = max(0, min(need - floor, pool))
            content.limit, pool, truncated = floor + extra, pool - extra, True
            if content.limit < 512:
                content.limit = 0
        return truncated


def span_entry(span, redactor, role, model):
    from opentelemetry.proto.trace.v1.trace_pb2 import Span
    from mlflow.tracing.utils.otlp import _set_otel_proto_anyvalue

    kind = json.loads(span.attributes.get('mlflow.spanType', '"CHAIN"'))
    if kind not in {'LLM', 'CHAT_MODEL', 'TOOL', 'AGENT', 'CHAIN', 'RETRIEVER'}:
        kind = 'CHAIN'
    result = Span(trace_id=span.context.trace_id.to_bytes(16, 'big'),
                  span_id=span.context.span_id.to_bytes(8, 'big'),
                  parent_span_id=span.parent.span_id.to_bytes(8, 'big') if span.parent else b'',
                  start_time_unix_nano=span.start_time, end_time_unix_nano=span.end_time)
    result.status.code = 2 if span.status.status_code.name == 'ERROR' else 1
    result.attributes.add(key='mlflow.spanType').value.string_value = kind
    result.attributes.add(key='tl3.role').value.string_value = redactor.text(role)[:256]
    if kind in {'LLM', 'CHAT_MODEL'}:
        result.attributes.add(key='mlflow.llm.model').value.string_value = redactor.text(model)[:256]
    # Retain only content and usage. Headers, provider metadata, resources and
    # raw exceptions stay local. Content is bounded now and redacted at export.
    if (raw := span.attributes.get('tl3.proxy.usage_chunks')) is not None:
        # Numeric-only usage has its own boundary; names such as
        # prompt_tokens must survive the general sensitive-key filter.
        value = [chunk_counts(chunk) for chunk in json.loads(raw)[:32]]
        _set_otel_proto_anyvalue(result.attributes.add(key='tl3.proxy.usage_chunks').value, value)
    if span.attributes.get('tl3.proxy.usage_chunks_truncated') is not None:
        result.attributes.add(key='tl3.proxy.usage_chunks_truncated').value.bool_value = True
    if kind in {'LLM', 'CHAT_MODEL'} and (raw := span.attributes.get('mlflow.chat.tokenUsage')):
        usage = json.loads(raw)
        usage = {key: value for key in TOKEN_FIELDS
                 if type(value := usage.get(key)) is int and value >= 0}
        attr = result.attributes.add(key='mlflow.chat.tokenUsage')
        for key, value in usage.items():
            attr.value.kvlist_value.values.add(key=key).value.int_value = value
    contents, inputs = [], None
    for key in ('mlflow.spanInputs', 'mlflow.spanOutputs'):
        if (raw := span.attributes.get(key)) is None:
            continue
        value = json.loads(raw)
        if key == 'mlflow.spanInputs':
            inputs = value
        elif kind == 'TOOL' and (path := sensitive_path(inputs)):
            value = file_marker(path)
        contents.append(Content(key, redactor.shape(value)))
    name = f'tl3_{role}:{model}' if kind in {'LLM', 'CHAT_MODEL'} else span.name
    return Entry(result, name, kind, contents)


def safe_span(span, redactor=None, role="unknown", model="unknown", payload_limit=PAYLOAD_BYTES):
    from mlflow.tracing.utils.otlp import _set_otel_proto_anyvalue

    redactor = redactor or Redactor(None)
    entry = span_entry(span, redactor, role, model)
    entry.proto.name = redactor.text(entry.name)[:256]
    for content in entry.contents:
        value = (redactor.clean(content.value, payload_limit) if payload_limit >= 512
                 else {'truncated': True, 'note': 'trace content budget exhausted'})
        _set_otel_proto_anyvalue(entry.proto.attributes.add(key=content.key).value, value)
    return entry.proto


def failure_tags(error):
    # Only fixed diagnostics may accompany the exception class. Provider errors
    # and validation errors can embed credentials, repository text or replies.
    safe_messages = {
        "done requires code changes", "could not inspect staged changes",
        "maker requires the worktree root", "missing final maker reply",
        "missing final model reply", "reply cites a memory it was not given",
        "only code may supply a forecast", "sized briefs must declare their root files",
        "duplicate JSON key", "non-finite JSON number",
        "ingest input exceeds exploration budget", "audit input exceeds exploration budget",
        "maker input exceeds exploration budget",
        "declared files must use canonical relative paths",
        "declared file exceeds the 1048576-byte limit",
        "declared file must be UTF-8 text without binary control bytes",
        "o200k_base tokenizer data must be preloaded in TIKTOKEN_CACHE_DIR",
        "fit limit must be smaller than context size",
    }
    message = "worker run failed"
    from jsonschema import ValidationError
    if isinstance(error, json.JSONDecodeError):
        message = f"invalid JSON at line {error.lineno}, column {error.colno}"
    elif isinstance(error, ValidationError):
        # Validator names and schema paths come from our schemas, never instance values.
        path = "/".join(str(part) for part in error.absolute_schema_path)
        message = f"schema validation failed: {error.validator} at schema {path}"
    elif isinstance(error, ValueError):
        message = "invalid worker data"
        if (type(error) is ValueError and len(error.args) == 1
                and isinstance(error.args[0], str) and error.args[0] in safe_messages):
            message = error.args[0]
    elif isinstance(error, OSError):
        message = "worker I/O failed"
    elif isinstance(error, subprocess.CalledProcessError):
        message = "worker subprocess failed"
    return {"failure_class": type(error).__name__[:64], "failure_message": message[:512]}


@contextmanager
def observe(request):
    capture = None
    if 'mlflow' not in request and (config := os.environ.get('TL3_MLFLOW_CONFIG')):
        try:
            request = {**request, 'mlflow': tomllib.loads(Path(config).read_text())['mlflow']}
        except Exception:
            failure()
    if request.get('mlflow'):
        try:
            os.environ['MLFLOW_DISABLE_TELEMETRY'] = 'true'
            logger = logging.getLogger('mlflow')
            logger.handlers = [logging.NullHandler()]
            logger.propagate = False
            capture = Capture(request)
        except Exception:
            failure()
    token = active.set(capture)
    outcome = 'failed'
    try:
        yield
        outcome = 'succeeded'
    except BaseException as error:
        if capture is not None:
            diagnostics = failure_tags(error)
            capture.tags.update(diagnostics)
            record_reply(diagnostics)
        raise
    finally:
        if capture is not None:
            try:
                capture.finish(outcome)
            except Exception:
                failure()
        active.reset(token)


def export():
    import signal
    import httpx

    def expired(*_):
        raise TimeoutError

    signal.signal(signal.SIGALRM, expired)
    signal.alarm(5)
    try:
        settings = json.loads(sys.stdin.buffer.readline())
        packet = sys.stdin.buffer.read()
        with httpx.Client(base_url=settings['tracking_uri'], timeout=1, trust_env=False) as client:
            response = client.get('/api/2.0/mlflow/experiments/get-by-name',
                                  params={'experiment_name': settings['experiment']})
            if response.status_code == 404:
                response = client.post('/api/2.0/mlflow/experiments/create',
                                       json={'name': settings['experiment']})
                # Another run may have created this experiment concurrently.
                if response.status_code == 400:
                    response = client.get('/api/2.0/mlflow/experiments/get-by-name',
                                          params={'experiment_name': settings['experiment']})
            response.raise_for_status()
            data = response.json()
            experiment = data.get('experiment', data)['experiment_id']
            try:
                client.post('/api/2.0/mlflow/experiments/set-experiment-tag',
                            json={'experiment_id': experiment, 'key': 'mlflow.experimentKind',
                                  'value': 'genai_development'})
            except httpx.HTTPError:
                pass  # Cosmetic UI tag must not discard a trace.
            response = client.post('/v1/traces', content=packet,
                                   headers={'Content-Type': 'application/x-protobuf',
                                            'x-mlflow-experiment-id': experiment})
            response.raise_for_status()
        return 0
    except Exception:
        failure()
        return 1
    finally:
        signal.alarm(0)


if __name__ == '__main__':
    raise SystemExit(export())
