// Reuse the real Controller and recorded GitHub fixture without changing queue scenarios.
#[allow(dead_code)]
mod fixture {
    include!("mergify.rs");
    include!("direct_merge/scenarios.rs");
}
