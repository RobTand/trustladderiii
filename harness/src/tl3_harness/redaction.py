"""Shared stdlib redactor for the harness and trace exporter; policy comes from Rust."""
import json
import re
from urllib.parse import parse_qsl, urlsplit

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



def azure_sas_spans(text):
    """Recognize SAS syntax while retaining raw source character coordinates."""
    # Match Rust char::is_whitespace, including its distinction from Python's
    # additional U+001C..U+001F string whitespace. This is URL syntax only.
    whitespace = '\t\n\v\f\r \x85\xa0\u1680\u2000\u2001\u2002\u2003\u2004\u2005\u2006\u2007\u2008\u2009\u200a\u2028\u2029\u202f\u205f\u3000'
    scanned_until = 0
    for match in re.finditer('://', text):
        colon = match.start()
        if colon < scanned_until:
            continue
        start = next((colon - len(scheme) for scheme in ('https', 'http')
                      if colon >= len(scheme) and text[colon - len(scheme):colon].lower() == scheme), None)
        if start is None or start and (text[start - 1].isalnum() or text[start - 1] == '_'):
            continue
        end = next((index for index in range(start, len(text))
                    if text[index] in whitespace or text[index] in '\"\'<>\\)]}'), len(text))
        scanned_until = end
        raw = text[start:end]
        try:
            parsed = urlsplit(raw)
            # Access port as well: url::Url refuses invalid ports during parsing.
            _ = parsed.port
            host = parsed.hostname
            if parsed.username is not None or parsed.password is not None or not host:
                continue
        except ValueError:
            continue
        suffix = '.blob.core.windows.net'
        if not host.endswith(suffix):
            continue
        account = host[:-len(suffix)]
        if not 1 <= len(account) <= 63 or any(char not in 'abcdefghijklmnopqrstuvwxyz0123456789' for char in account):
            continue
        query_end = raw.find('#') if '#' in raw else len(raw)
        question = raw.find('?', 0, query_end)
        if question < 0:
            continue
        offset, fields, valid = question + 1, {}, True
        for pair in raw[offset:query_end].split('&'):
            if '=' in pair:
                key, value = pair.split('=', 1)
                if len(key.encode()) > 4096 or len(value.encode()) > 4096 or re.search(r'%(?![0-9A-Fa-f]{2})', pair):
                    valid = False
                    break
                decoded = parse_qsl(pair, keep_blank_values=True)
                if not decoded:
                    valid = False
                    break
                key, value = decoded[0]
                if key in {'sv', 'se', 'ss', 'sr', 'sig'}:
                    span = start + offset + pair.index('=') + 1, start + offset + len(pair)
                    if not value.strip(whitespace) or key in fields:
                        valid = False
                        break
                    fields[key] = span
            offset += len(pair) + 1
        if valid and 'sv' in fields and 'se' in fields and ('ss' in fields or 'sr' in fields) and 'sig' in fields:
            yield fields['sig']


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
        found = list(azure_sas_spans(text))
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


