"""UNREPLACED slice-6 exception: frozen supervision.py requires Redactor.window.

Only its span/window dependency closure remains until Flux executor verified.
Every other caller uses the Rust action policy.
"""
import json
import re

from ._redaction_policy import POLICY

PAYLOAD_BYTES = 65536
STRING_CHARS = 8192
ITEM_BYTES = 16384
LEAD_NOTE = '[truncated: earlier text omitted]\n'

CONTEXT = POLICY['context']
REDACTED = POLICY['redacted']
LABEL_WORDS = re.compile(POLICY['regex']['LABEL_WORDS'])
REFERENCE = re.compile(POLICY['regex']['REFERENCE'])
TEMPLATE = re.compile(POLICY['regex']['TEMPLATE'])
SCHEME_NAME = re.compile(POLICY['regex']['SCHEME_NAME'])
PATH_SEGMENT = re.compile(POLICY['regex']['PATH_SEGMENT'])
VERSION = re.compile(POLICY['regex']['VERSION'])
CODE_NAME = re.compile(POLICY['regex']['CODE_NAME'])
PATHLIKE = re.compile(POLICY['regex']['PATHLIKE'])
DOTTED = re.compile(POLICY['regex']['DOTTED'])
NUMBER = re.compile(POLICY['regex']['NUMBER'])
IDENTIFIER = re.compile(POLICY['regex']['IDENTIFIER'])
WORD = re.compile(POLICY['regex']['WORD'])
LABELLED = re.compile(POLICY['regex']['LABELLED'])
FLAG = re.compile(POLICY['regex']['FLAG'])
TABLE = re.compile(POLICY['regex']['TABLE'])
BLOCK_SCALAR = re.compile(POLICY['regex']['BLOCK_SCALAR'])
NAME_VALUE = re.compile(POLICY['regex']['NAME_VALUE'])
SECTION = re.compile(POLICY['regex']['SECTION'])
SECTION_VALUE = re.compile(POLICY['regex']['SECTION_VALUE'])
AWS_SET = re.compile(POLICY['regex']['AWS_SET'])
AUTH_HEADER = re.compile(POLICY['regex']['AUTH_HEADER'])
SCHEME = re.compile(POLICY['regex']['SCHEME'])
BEARER = re.compile(POLICY['regex']['BEARER'])
PRIVATE_HEADER = re.compile(POLICY['regex']['PRIVATE_HEADER'])
PRIVATE_END = re.compile(POLICY['regex']['PRIVATE_END'])
KEY_LINES = re.compile(POLICY['regex']['KEY_LINES'])
TOKEN_CHAR = re.compile(POLICY['regex']['TOKEN_CHAR'])
NOT_TOKEN = re.compile(POLICY['regex']['NOT_TOKEN'])
CANDIDATE = re.compile(POLICY['regex']['CANDIDATE'])
WORDISH = re.compile(POLICY['regex']['WORDISH'])
SAFE_TOKEN = re.compile(POLICY['regex']['SAFE_TOKEN'])
SENSITIVE_NAME = re.compile(POLICY['regex']['SENSITIVE_NAME'])
SENSITIVE_BROAD = re.compile(POLICY['regex']['SENSITIVE_BROAD'])
SOURCE_FILE = re.compile(POLICY['regex']['SOURCE_FILE'])
NEAR_WORD = re.compile(POLICY['regex']['NEAR_WORD'])
NEAR_TOKEN = re.compile(POLICY['regex']['NEAR_TOKEN'])
SECRET_DATA = re.compile(POLICY['regex']['SECRET_DATA'])
TOKEN_FIELDS = tuple(POLICY['words']['TOKEN_FIELDS'])
SECRET_PARTS = tuple(POLICY['words']['SECRET_PARTS'])
SECRET_WORDS = set(POLICY['words']['SECRET_WORDS'])
WEAK_WORDS = set(POLICY['words']['WEAK_WORDS'])
KEY_QUALIFIERS = set(POLICY['words']['KEY_QUALIFIERS'])
NOT_SECRET = set(POLICY['words']['NOT_SECRET'])
PLACEHOLDERS = set(POLICY['words']['PLACEHOLDERS'])
KEYWORDS = set(POLICY['words']['KEYWORDS'])
TYPE_NAMES = set(POLICY['words']['TYPE_NAMES'])
PATH_KEYS = set(POLICY['words']['PATH_KEYS'])
CONTENT_KEYS = set(POLICY['words']['CONTENT_KEYS'])
SKIPPED_KEYS = set(POLICY['words']['SKIPPED_KEYS'])
HELPERS = {name: re.compile(pattern) for name, pattern in POLICY['helpers'].items()}
PATTERNS = [re.compile(pattern) for pattern in POLICY['patterns']]


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
    stripped = HELPERS['UNQUOTE_PREFIX'].sub('', value).lstrip('\\')
    if stripped[:1] in ('"', "'", '`') and len(stripped) >= 2:
        return stripped[1:-1].rstrip('\\'), True
    return value, False


def placeholder(value):
    """Words only, one of which says the value is not real, as in fake-token."""
    return (HELPERS['PLACEHOLDER_TEXT'].fullmatch(value) is not None
            and bool(PLACEHOLDERS.intersection(word.lower() for word in LABEL_WORDS.findall(value))))


def plausible(kind, label, value, after='', assign='', code=False):
    """Whether a labelled value can be a secret rather than code, a type, prose or a fixture."""
    inner, quoted = unquote(value)
    if (not kind or not inner.strip() or REFERENCE.match(inner) or PATHLIKE.fullmatch(inner)
            or TEMPLATE.search(inner) or VERSION.fullmatch(inner) or inner.lower() == label.lower()
            or SCHEME_NAME.fullmatch(inner.strip()) or placeholder(inner[:64])):
        return False
    if quoted and (after.lstrip()[:1] in ('*', '+') or HELPERS['CONCATENATION'].match(inner)):
        return False  # 'b' * 64, or the end of one string literal and the start of the next
    if not quoted:
        if (inner in KEYWORDS or inner in TYPE_NAMES or any(c in inner for c in '{([') or inner[0] in '&-'
                or (after and after[0] in '([') or DOTTED.fullmatch(inner) or NUMBER.fullmatch(inner)):
            return False
        if assign == ':' and HELPERS['TYPE_IDENTIFIER'].fullmatch(inner) and HELPERS['TYPE_TAIL'].match(after):
            return False
        if IDENTIFIER.fullmatch(inner) and label_kind(inner):
            return False
        if CODE_NAME.fullmatch(inner) and code:
            return False
        if WORD.fullmatch(inner) and HELPERS['WORD_TAIL'].match(after):
            return False
        if HELPERS['SHORT_NAME'].fullmatch(inner) and HELPERS['LOWER_TAIL'].match(after):
            return False
    if kind == 'weak':
        # Mixed upper case, lower case and digits in one run, as in DB_PASS=CVwg8aolbgRxKBOL.
        sample = inner[:64]
        return random_like(sample, 12) or (
            HELPERS['WEAK_TOKEN'].fullmatch(sample) is not None and any(map(str.isupper, sample))
            and any(map(str.islower, sample)) and any(map(str.isdigit, sample)))
    return True


def token_shaped(value):
    return not WORD.fullmatch(value) and (any(map(str.isdigit, value)) or any(c in '._~+/=-' for c in value)
                                          or any(map(str.isupper, value[1:])))


def random_like(token, minimum=20):
    """Frequent switches between upper case, lower case and digits, or few readable words."""
    if SAFE_TOKEN.fullmatch(token) or any(map(str.isspace, token)):
        return False
    parts = HELPERS['TOKEN_PARTS'].split(token)
    if len(parts) > 1 and all(HELPERS['ORDINARY_PART'].fullmatch(part) or len(part) <= 2 for part in parts):
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
    if WORD.fullmatch(value) and len(HELPERS['PROSE_WORDS'].findall(text[line_start:match.start()])) >= 2:
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
    return HELPERS['READABLE_TEXT'].fullmatch(inner) is not None and not random_like(inner, 8)


def check_header(match, text):
    group = 'v' if match['v'] is not None else 'c'
    value = match[group]
    inner = unquote(value)[0]
    if (REFERENCE.match(inner) or TEMPLATE.search(inner) or placeholder(inner)
            or HELPERS['HEADER_REFERENCE'].match(value)):
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
            or HELPERS['NUMBER_TEXT'].fullmatch(core) or placeholder(core)):
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


CHECKED = [(re.compile(pattern), globals()[check]) for pattern, check in POLICY['checked']]
NEAR_CHARS = POLICY['near_chars']


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
        elif 2 <= len(cells) <= 64 and all(HELPERS['CSV_HEADER'].fullmatch(cell) for cell in cells):
            columns = {index for index, cell in enumerate(cells) if label_kind(cell.strip()) == 'strong'}
            width = len(cells)
        else:
            columns = None
        position += len(line) + 1
    return found


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
            if text.startswith(('...', '\u2026'), match.end()) and HELPERS['HEX_PREFIX'].fullmatch(token):
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
            for piece in HELPERS['PATH_PARTS'].split(token):
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

