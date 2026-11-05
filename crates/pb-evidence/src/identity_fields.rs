#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

/// #821 / Q55: the bound identity retains each declared component independently.
#[cfg_attr(not(verus_keep_ghost), derive(serde::Serialize))]
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
pub struct BoundIdentity<T> {
    pub base_sha: T,
    pub selectors: T,
    pub forcing: T,
    pub declared_symptom: T,
}

/// #821: JSON encoding operates on this exact field projection; no field is discarded.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(out => ensures
    out.base_sha == base_sha,
    out.selectors == selectors,
    out.forcing == forcing,
    out.declared_symptom == declared_symptom,
))]
pub fn bind<T>(base_sha: T, selectors: T, forcing: T, declared_symptom: T) -> BoundIdentity<T> {
    BoundIdentity {
        base_sha,
        selectors,
        forcing,
        declared_symptom,
    }
}
