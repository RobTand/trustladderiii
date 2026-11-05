use vstd::prelude::*;
#[path = "../src/identity_fields.rs"]
mod identity_fields;
#[path = "../src/decision.rs"]
mod decision;
verus! {
/// #821 / Q55: field projection is injective and deterministic over every u64 component.
/// Components represent the parser/serializer-tested JSON trees, not a hash-injectivity claim.
fn projection(a: (u64,u64,u64,u64), b: (u64,u64,u64,u64)) {
    let x = identity_fields::bind(a.0,a.1,a.2,a.3);
    let y = identity_fields::bind(b.0,b.1,b.2,b.3);
    assert((x.base_sha == y.base_sha && x.selectors == y.selectors && x.forcing == y.forcing && x.declared_symptom == y.declared_symptom) == (a == b));
}
/// #821 / Q55: real patch-reordering predicate preserves alias and parent dependencies.
fn identity_witness() {
    let value = decision::independent(false,false,false);
    assert(value);
    let value = decision::independent(true,false,false);
    assert(!value);
    let value = decision::independent(false,true,false);
    assert(!value);
    let value = decision::independent(false,false,true);
    assert(!value);
}
}
fn main() {}
