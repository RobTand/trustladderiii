"""Regenerate the shared intake, maker and reviewer obligation fields."""
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2] / 'workers/src/tl3_workers/schemas'


def main():
    text = {'type': 'string', 'minLength': 1, 'pattern': r'\S'}
    publication = {
        'type': 'object',
        'properties': {
            'branch': text,
            'branch_prefix': text,
            'non_draft': {'const': True},
            'body_lines': {'type': 'array', 'uniqueItems': True, 'items': {'type': 'string', 'minLength': 1, 'maxLength': 512, 'pattern': r'^[^\r\n]+$'}},
            'proof': {'type': 'boolean'},
        },
        'required': ['non_draft', 'body_lines', 'proof'],
        'additionalProperties': False,
    }
    for name in ('ingest-reply', 'maker-input', 'reviewer-input'):
        path = ROOT / f'{name}.json'
        document = json.loads(path.read_text())
        container = document['oneOf'][0] if name == 'ingest-reply' else document
        goal = container['properties']['goals']['items']
        goal['properties'].update(stage={'enum': ['maker', 'checking', 'publishing']}, publication=publication)
        goal['allOf'] = [
            {'if': {'required': ['stage'], 'properties': {'stage': {'const': 'publishing'}}},
             'then': {'required': ['publication']},
             'else': {'not': {'required': ['publication']}}},
        ]
        path.write_text(json.dumps(document, indent=2) + '\n')


if __name__ == '__main__':
    main()
