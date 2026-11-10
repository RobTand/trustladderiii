#!/usr/bin/env python3
"""Generate worker A's named tests and coverage inventory from pinned Kani text."""
from pathlib import Path
import hashlib
import json
import re

HERE = Path(__file__).resolve().parent
KERNEL = HERE.parents[1]
proofs = KERNEL / 'src/proofs.rs'
source = proofs.read_text()
names = re.findall(r'proof!\(\s*([a-z_][a-z0-9_]*)\s*,', source)
assert len(names) == 78
selected = [(i + 1, n) for i, n in enumerate(names) if 6 <= i + 1 <= 56]
events, advances = [], []
lines = ['//! The same assertions on enumerated and sampled inputs.',
         '#[path = "kani_scenarios/common/mod.rs"]', 'mod common;',
         'use common::*;', '#[path = "kani_scenarios/common/a_extra.rs"]', 'mod a_extra;', '']
advance_phases = {
    'ingesting': '[Phase::Ingesting]', 'auditing': '[Phase::Auditing]',
    'unplanned': '[Phase::Unplanned]',
    'planning': '[Phase::Planning { must_split: false }, Phase::Planning { must_split: true }]',
    'ready': '[Phase::Ready(Resume::Making), Phase::Ready(Resume::Checking), Phase::Ready(Resume::Reviewing), Phase::Ready(Resume::Publishing), Phase::Ready(Resume::Rebasing)]',
    'making': '[Phase::Making]', 'checking': '[Phase::Checking]',
    'reviewing': '[Phase::Reviewing]', 'publishing': '[Phase::Publishing]',
    'awaiting_delivery': '[Phase::AwaitingDelivery]', 'rebasing': '[Phase::Rebasing]',
    'stopping': '[Phase::Stopping(Outcome::Merged), Phase::Stopping(Outcome::Stopped), Phase::Stopping(Outcome::Replaced), Phase::Stopping(Outcome::Duplicate), Phase::Stopping(Outcome::AlreadyDone), Phase::Stopping(Outcome::NotActionable)]',
}
manifest = []
for hid, name in selected:
    if name.startswith('apply_'):
        call = f'a_extra::run_event("kani_{name}", "{name[6:]}");'
        events.append(name)
        family = 'event'
    else:
        phase = name[8:]
        call = f'a_extra::run_advance("kani_{name}", &{advance_phases[phase]}, {str(phase != "awaiting_delivery").lower()});'
        advances.append(name)
        family = 'advance'
    lines.extend(['#[test]', f'fn kani_{name}() {{', f'    {call}', '}', ''])
    manifest.append({'id': f'H{hid}', 'name': name, 'test': f'kani_{name}', 'family': family})
assert len(events) == 39 and len(advances) == 12
(KERNEL / 'tests/kani_events_advance.rs').write_text('\n'.join(lines))
(HERE / 'a_inventory.json').write_text(json.dumps({
    'source': 'crates/kernel/src/proofs.rs', 'source_sha256': hashlib.sha256(proofs.read_bytes()).hexdigest(),
    'seed': '0x4b3511105eeda001', 'harnesses': manifest,
    'claim': 'the same assertions on enumerated and sampled inputs',
}, indent=2) + '\n')
print(json.dumps({'event_tests': len(events), 'advance_tests': len(advances)}))
