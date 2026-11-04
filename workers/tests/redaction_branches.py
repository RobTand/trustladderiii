"""#753 differential cases and executable coverage of each policy-pattern alternative."""
import json
import re
from pathlib import Path

from tracing_forms import FORMS, PLANTED, PLANTED_LINES
from tracing_forms3 import FORMS as MORE

FAKE = 'zE9pR4cV7x'
HEX = '0123456789abcdef'


def original_inputs():
    return [text for _, text, *_ in FORMS + MORE] + [text for text, _ in PLANTED + PLANTED_LINES]


def branch_inputs():
    alnum = (FAKE * 8)[:64]
    hex_ = HEX * 4
    providers = [prefix + alnum for prefix in [
        'sk-', 'sk_live_', 'sk_test_', 'rk_live_', 'rk_test_', 'whsec_',
        *['gh' + c + '_' for c in 'pousr'], 'github_pat_',
        *['gl' + kind + '-' for kind in ['pat', 'rt', 'ptt', 'dt', 'cbt', 'soat', 'ft', 'imt', 'oas']],
        *['xox' + c + '-' for c in 'abeprs'], 'xapp-', 'pypi-', 'npm_', 'hf_', 'AIza',
        'ya29.', 'GOCSPX-', '1//0', 'dckr_pat_', 'ATATT3', 'lin_api_', 'ntn_', 'secret_',
        *['hv' + c + '.' for c in 'sbr'],
    ]]
    providers += [prefix + 'AB12CD34EF56GH78' for prefix in ['AKIA', 'ASIA', 'AGPA', 'AIDA', 'AROA', 'ANPA']]
    providers += ['SG.' + alnum + '.' + alnum, 'SK' + hex_[:32], 'key-' + hex_[:32],
                  *['do' + c + '_v1_' + hex_ for c in 'opr'],
                  *['shp' + kind + '_' + hex_[:32] for kind in ['at', 'ca', 'pa', 'ss']],
                  'dapi' + hex_[:32], 'dapi' + hex_[:32] + '-3', 'sbp_' + hex_[:40],
                  'PMAK-' + hex_[:24] + '-' + hex_[:24], 'pat' + alnum[:14] + '.' + hex_,
                  alnum[:14] + '.atlasv1.' + alnum,
                  *[c + alnum[:23] + '.' + alnum[:6] + '.' + alnum[:27] for c in 'MNO']]
    inputs = providers + [
        'Private-Lines: 1\n' + alnum + '\nPrivate-MAC: fixture\n',
        'eyJfixture.eyJfixture.signature', '123456789:AA' + alnum,
        '.example.invalid\tTRUE\t/\tFALSE\t0\tfixture\t' + FAKE,
        '#HttpOnly_.example.invalid\tTRUE\t/\tFALSE\t0\tfixture\t' + FAKE,
        *['$' + kind + '$salt$' + alnum for kind in ['apr1', '1', '2', '5', '6', 'y', 'gy', '7']],
        '$2b$12$' + alnum[:53],
        "IDENTIFIED WITH fixture BY PASSWORD ('" + FAKE + "')",
        'IDENTIFIED BY "' + FAKE + '"',
        *['https://' + host + '/' + alnum for host in [
            'hooks.slack.com/services', 'discord.com/api/webhooks', 'discordapp.com/api/webhooks',
            'outlook.office.com/webhook', 'outlook.office365.com/webhook',
            'fixture.webhook.office.com/webhookb2']],
        'sshpass -p ' + FAKE,
        *[tool + ' registry login -p ' + FAKE for tool in ['docker', 'podman', 'buildah', 'helm', 'skopeo']],
        *[tool + ' -p' + FAKE for tool in ['mysql', 'mysqldump', 'mysqladmin', 'mariadb']],
        'htpasswd -b fixture user ' + FAKE,
        *['openssl -pass' + suffix + ' pass:' + FAKE for suffix in ['', 'in', 'out']],
        *[tool + ' -P ' + FAKE for tool in ['unzip', 'zip']],
        '7za -p' + FAKE, 'login fixture password ' + FAKE,
        'password ' + FAKE + '\n', 'fixture:1000:x:x:' + FAKE,
    ]
    return inputs


def instrument(pattern):
    """Wrap each syntactic | arm in a named capture, without changing its predicate."""
    labels = []
    index = 0
    prefix = ''
    while match := re.match(r'\(\?[aiLmsux]+\)', pattern[index:]):
        prefix += match[0]
        index += len(match[0])

    def expression(nested=False):
        nonlocal index
        arms = ['']
        while index < len(pattern):
            char = pattern[index]
            index += 1
            if char == '\\':
                arms[-1] += char + pattern[index]
                index += 1
            elif char == '[':
                start = index - 1
                while index < len(pattern):
                    c = pattern[index]
                    index += 1
                    if c == '\\':
                        index += 1
                    elif c == ']':
                        break
                arms[-1] += pattern[start:index]
            elif char == '(':
                start = index - 1
                if pattern.startswith('?P=', index):
                    index = pattern.index(')', index) + 1
                    arms[-1] += pattern[start:index]
                    continue
                header = re.match(r'\?(?:P<[^>]+>|[:=!]|<[=!]|>|[aiLmsux-]+:)', pattern[index:])
                opening = '(' + (header[0] if header else '')
                if header:
                    index += len(header[0])
                arms[-1] += opening + expression(True) + ')'
            elif char == ')' and nested:
                break
            elif char == '|':
                arms.append('')
            else:
                arms[-1] += char
        if len(arms) == 1:
            return arms[0]
        result = []
        for arm in arms:
            name = 'coverage753_' + str(len(labels))
            labels.append(name)
            result.append('(?P<' + name + '>' + arm + ')')
        return '|'.join(result)

    translated = prefix + expression()
    assert index == len(pattern)
    return translated, labels


def predicate_cases(policy):
    cases = []
    bases = {
        'SCHEME_NAME': ('basic', 'full'), 'SCHEME': ('Basic ' + FAKE, 'match'),
        'BEARER': ('Basic ' + FAKE, 'search'),
        'AUTH_HEADER': ('Authorization: Basic ' + FAKE, 'search'),
        'NEAR_WORD': ('credentials ' + FAKE, 'search'),
        'SAFE_TOKEN': ('prismabuild-job' + HEX * 2, 'full'),
        'SENSITIVE_NAME': ('service-account.json', 'full'),
        'SENSITIVE_BROAD': ('credentials/', 'search'),
        'SOURCE_FILE': ('.swift', 'search'),
    }
    bases.update({'patterns:7': ("IDENTIFIED BY '" + FAKE + "'", 'search'),
                  'patterns:8': ('https://discord.com/api/webhooks/1/' + FAKE, 'search'),
                  'checked:5': ('Cookie: sid=' + FAKE, 'search'),
                  'checked:6': ('Basic ' + FAKE, 'search'),
                  'checked:15': ('login: fixture / ' + FAKE, 'search')})
    for name, (text, mode) in bases.items():
        if ':' in name:
            collection, index = name.split(':')
            pattern = policy[collection][int(index)]
            if collection == 'checked':
                pattern = pattern[0]
        else:
            pattern = policy['regex'][name]
        for letter in 'iIİı':
            changed = ''.join(letter if c in 'iI' else c for c in text)
            cases.append((name + ':' + letter, pattern, changed, mode))
    cases += [('scope', r'(?i:i(?-i:I)i)', text, 'full') for text in ['ıIİ', 'ıiİ']]
    cases += [('class', pattern, letter, 'full')
              for pattern in [r'(?i)[a-z]', r'(?i)[^a-z]', r'(?i)[I]', r'(?i)[\wI]', r'[I]']
              for letter in 'iIİı']
    return cases


def anchor_cases(policy):
    cases = [('anchor', pattern, text, 'search')
             for pattern in [r'x\Z', r'x$', r'(?m)x$', r'(?m:x$)', r'(?m:(?-m:x$))', r'(?m)^x$']
             for text in ['x', 'x\n', 'x\n\n', 'x\r\n', 'x\ny', '\nx\n']]
    inputs = original_inputs() + branch_inputs()
    for collection in ['regex', 'helpers', 'patterns', 'checked']:
        entries = policy[collection]
        entries = entries.items() if isinstance(entries, dict) else enumerate(entries)
        for key, value in entries:
            pattern = value[0] if collection == 'checked' else value
            if '$' not in pattern and r'\Z' not in pattern:
                continue
            examples = [m[0] for text in inputs for m in re.finditer(pattern, text)][:5]
            for example in examples:
                for suffix in ['', '\n', '\n\n', '\r\n']:
                    cases.append((str(key), pattern, example + suffix, 'search'))
    cases += [('HEADER_REFERENCE', policy['helpers']['HEADER_REFERENCE'], 'value\n', 'match'),
              ('SOURCE_FILE', policy['regex']['SOURCE_FILE'], '.swift\n', 'search')]
    return cases


def encode_cases(cases):
    result = []
    for label, pattern, text, mode in cases:
        regex = re.compile(pattern)
        match = (regex.fullmatch(text) if mode == 'full' else regex.match(text) if mode == 'match'
                 else regex.search(text))
        expected = None if match is None else {'0': match[0], **match.groupdict()}
        result.append({'label': label, 'pattern': pattern, 'text': text, 'mode': mode, 'expected': expected})
    return result


def coverage_cases(policy):
    inputs = original_inputs() + branch_inputs()
    result = []
    for index, pattern in enumerate(policy['patterns']):
        instrumented, branches = instrument(pattern)
        regex = re.compile(instrumented)
        witnesses = {}
        for text in inputs:
            for match in regex.finditer(text):
                for branch in branches:
                    if match[branch] is not None:
                        witnesses.setdefault(branch, text)
        result.append({'index': index, 'pattern': instrumented, 'branches': branches,
                       'witnesses': witnesses, 'matched': any(re.search(pattern, text) for text in inputs)})
    return result


def main():
    import sys
    policy = json.loads(Path(sys.argv[1]).read_text())
    print(json.dumps({'folding': encode_cases(predicate_cases(policy)),
                      'anchors': encode_cases(anchor_cases(policy)),
                      'coverage': coverage_cases(policy)}))


if __name__ == '__main__':
    main()
