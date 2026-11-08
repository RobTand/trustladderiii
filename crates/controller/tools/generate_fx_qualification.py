#!/usr/bin/env python3
"""Generate the FX pin from hash-checked accepted records, never a candidate bundle."""
import argparse
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
INPUTS = ROOT / 'qualification/fx-exact42b'
OUTPUT = ROOT / 'src/adapters/flux_checks/activation/qualification.json'
SOURCE = '42b68e30fcb3d37cc03c0a70166af9e912b25dc6'
# These are retained source identities, not publisher provenance for future profiles.
RUNS = {
    'canary2': (
        '640fe3b8b535b1db142ad099e0bf115f6255f90923b3dd73fc681eaca7e93391',
        'e32cfdccf2a284c5446a53fbaf9699aba2e9937c7bd0aa0c9d1999ae78a08cbf',
        'f339f5bb17c9f48c77ed07a9a72348777f01397de260d55df35a18813aeca47a',
        '10a06385ce91404961e6b0d2894d0e75a7b24f21', '/opt/tl3/bin/python-canary2-absent'),
    'canary3': (
        '4fbf54b83553eaaafc0ea6376bd4e1c1382d3fd58e84a5c44e85c1d73dea61c3',
        '765f75dcc517039a31bdc73ae8846f1b3902c41725d4edfa3c06849c54943179',
        'f8577888c7db9cbc6b927adc501d899f5df5b746ff4775775bf044d43b175a09',
        '2a332ed2f7a6afbc6619e82802d378b6c9fb867a', '/opt/tl3/bin/python'),
    'success': (
        '294d58f0b5feb517d7b70426e0b292b74b91aad03b55483bb9a84e57feb9930a',
        '34515eb786c02fd1d1b14af5ddcc751d4b7e5f9f0ff3bdbf0451ed69e0707866',
        'f95dde156917aa7012e5c090e945e9ceb3748094724f5890adab49c9af7b17a4',
        '63da5ca7776ced528d6817e3dde46d52ce4634d5', '/opt/tl3/bin/python'),
}
PROFILE_FILES = ['image-inspect.json', 'image-map.json', 'preflight.json']


def checked(inputs, name, expected):
    raw = (inputs / name).read_bytes()
    if hashlib.sha256(raw).hexdigest() != expected:
        raise ValueError('qualification.input.sha256')
    return json.loads(raw)


def generate(inputs=INPUTS):
    ruling = checked(inputs, 'parent-reconciliation.json',
                     '0623e1deacca0adddba501822e433be68e7c5f1abcaeaa3fcfe8d96d485c03be')
    fault = checked(inputs, 'canary2-interpreter-provenance.json',
                    '0640df231230053922fc64d3bab8f154bc40c66d4f08dafd7d4423a8253faeed')
    if (fault['source'] != SOURCE or fault['observed_interpreter_claim'] is not False
            or fault['fault_input']['value'] != RUNS['canary2'][4]):
        raise ValueError('qualification.negative_fixture')
    manifests = {}
    provenance = []
    for name, (manifest_hash, acceptance_hash, audit_hash, producer, interpreter) in RUNS.items():
        manifest = checked(inputs, f'{name}-manifest.json', manifest_hash)
        acceptance = checked(inputs, f'{name}-acceptance.json', acceptance_hash)
        audit = checked(inputs, f'{name}-parent-audit.json', audit_hash)
        retained = next(run for run in ruling['runs'] if run['run'] == name)
        if (acceptance['status'] != 'PASS' or acceptance['source'] != SOURCE
                or audit['source'] != SOURCE
                or audit['status'] != f'PASS_PARENT_RETAINED_{name.upper()}_AUDIT'
                or manifest['producer_commit'] != producer
                or manifest['container_interpreter'] != interpreter
                or retained['manifest']['sha256'] != manifest_hash
                or retained['accepted_producer_provenance'] != producer
                or retained['accepted_interpreter'] != interpreter):
            raise ValueError('qualification.accepted_provenance')
        manifests[name] = manifest
        provenance.append({'run': name, 'manifest_sha256': manifest_hash,
                           'acceptance_sha256': acceptance_hash, 'parent_audit_sha256': audit_hash,
                           'producer_commit': producer, 'container_interpreter': interpreter,
                           'citations': retained['citations']})
    reference = manifests['canary3']
    for name, manifest in manifests.items():
        expected = dict(reference, producer_commit=RUNS[name][3], container_interpreter=RUNS[name][4])
        if manifest != expected:
            raise ValueError('qualification.execution_metadata')
    if len(reference['files']) != 37 or len({f['path'] for f in reference['files']}) != 37:
        raise ValueError('qualification.inventory')
    metadata = {key: value for key, value in reference.items() if key not in ('producer_commit', 'files')}
    result = {'schema': 'tl3.fx_qualification.v1', 'acceptance_source': SOURCE,
              'reconciliation_sha256': '0623e1deacca0adddba501822e433be68e7c5f1abcaeaa3fcfe8d96d485c03be',
              'historical_runs': provenance, 'metadata': metadata,
              'files': [f for f in reference['files'] if f['path'] not in PROFILE_FILES],
              'profile_files': PROFILE_FILES}
    return (json.dumps(result, indent=2, sort_keys=True) + '\n').encode()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, default=OUTPUT)
    parser.add_argument('--check', action='store_true')
    args = parser.parse_args()
    raw = generate()
    if args.check:
        if args.output.read_bytes() != raw:
            raise SystemExit('qualification.regeneration')
    else:
        args.output.write_bytes(raw)


if __name__ == '__main__':
    main()
