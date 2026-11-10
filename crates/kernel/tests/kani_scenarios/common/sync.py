#!/usr/bin/env python3
"""Refresh test scope and verbatim Kani assertion functions; no production edits."""
from pathlib import Path
import hashlib,json,re,sys
D=Path(__file__).resolve().parent
K=D.parents[2]
root=(K/'src/lib.rs').read_text()
proof=(K/'src/proofs.rs').read_text()
# Compile the actual private implementation modules in the test scope. Only
# lib.rs needs a scope copy, since Rust include! rejects its inner attributes.
lines=root.splitlines(keepends=True)
out=[]
i=0
while i<len(lines):
 line=lines[i]
 if line.startswith('//!'):
  out.append('//'+line[3:]);i+=1;continue
 if line.startswith('#![cfg_attr(not(any(kani, test)), no_std)]'):
  i+=1;continue
 # Avoid duplicating unrelated unit-test modules in every scenario binary.
 if line.startswith('#[cfg(all(test, not(verus_keep_ghost)))]'):
  i+=1
  while not re.match(r'mod \w+;',lines[i]):i+=1
  i+=1;continue
 if line.startswith('#[path = '):
  path=re.fullmatch(r'#\[path = "([^"]+)"\]\n',line)[1]
  out.append('#[path = "../../../src/'+path+'"]\n');i+=1;continue
 if re.fullmatch(r'(pub )?mod \w+;\n',line):
  if not out[-1].startswith('#[path = '):
   name=re.search(r'mod (\w+);',line)[1]
   # Out-of-line modules may have mod.rs rather than NAME.rs.
   path=name+'.rs' if (K/'src'/ (name+'.rs')).exists() else name+'/mod.rs'
   out.append('#[path = "../../../src/'+path+'"]\n')
  out.append(line);i+=1;continue
 line=re.sub(r'include!\("([^"]+)"\)',r'include!("../../../src/\1")',line)
 out.append(line);i+=1
scope=''.join(out)
(D/'kernel_scope.rs').write_text(scope)
(D/'kernel_original.txt').write_text(root)
names=['small_head','small','require_consistent','teardown','check_nodes','check_outputs','check_effect','rebase_progress_applies','check_apply','check_fail_fast','check_fail_fast_funding','check_fail_fast_diverts','check_advance','reply','waiting_dependency_world','check_dependency_delivery','check_dependency_waiting','check_dependency_cycles','check_dependency_registration','check_dependency_remove','ack','fact','command','answer','adoption_preparation','check_adoption_at_most_once','check_adoption_has_no_maker_credit','gate_world','gate_world_in_phase','gate_world_with_waiters','check_gate_grant','check_gate_planning','publishing_ci_transition','check_publishing_ci_tests','check_publishing_ci_earlier_step','check_gate_leaf_progress','spec_key','pool_world','check_pool_grant_order','check_pool_order_is_strict_priority']
rows=[]
functions=[]
for name in names:
 m=re.search(r'^fn '+name+r'\([^\n]*(?:\n(?!}).*)*?\n}',proof,re.M)
 assert m, name
 text=m[0]
 assert re.search(r'^fn '+name+r'\(',text,re.M)
 functions.append(text+'\n')
 rows.append({'function':name,'source':'crates/kernel/src/proofs.rs','line':proof[:m.start()].count('\n')+1,'source_sha256':hashlib.sha256(text.encode()).hexdigest(),'copy_sha256':hashlib.sha256(text.encode()).hexdigest(),'status':'BYTE_IDENTICAL'})
header='''//! The same assertions on enumerated and sampled inputs.
#![allow(dead_code, unused_imports)]
include!("kernel_scope.rs");
mod sampling;
// This is a test-local deterministic compatibility namespace, not the Kani crate.
// Keeping these bindings preserves every copied assertion and filter byte for byte.
use sampling as kani;
const LIMIT: u64 = 1 << 40;
const CREDIT_LIMIT: u64 = 1 << 30;
'''
(D/'mod.rs').write_text(header+'\n'+'\n'.join(functions)+'\ninclude!("api.rs");\n')
(D/'assertion_hashes.json').write_text(json.dumps({'source_sha256':hashlib.sha256(proof.encode()).hexdigest(),'root_source_sha256':hashlib.sha256(root.encode()).hexdigest(),'test_scope_sha256':hashlib.sha256(scope.encode()).hexdigest(),'functions':rows},indent=2)+'\n')
print(json.dumps({'copied_functions':len(rows),'all_equal':all(r['source_sha256']==r['copy_sha256'] for r in rows)}))
