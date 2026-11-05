//! #704/#723/#823: exact ordered selector identities, including Python surrogates.

#[cfg(not(verus_keep_ghost))]
mod runtime {
    use super::identity_fields_match;
    use crate::summary::PythonText;
    use schemars::JsonSchema;
    use serde::{Deserialize, Serialize};
    use sha2::{Digest, Sha256};

    pub const SCHEMA: &str = "tl3.selectors.v1";

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
    #[serde(deny_unknown_fields)]
    pub struct SelectorIdentity {
        pub schema: String,
        pub count: usize,
        pub sha256: String,
        pub prefix: Vec<PythonText>,
    }

    /// Hash compact ensure_ascii JSON, preserving Python order and duplicates.
    pub fn selector_identity(selectors: &[PythonText]) -> SelectorIdentity {
        let mut bytes = vec![b'['];
        for (index, selector) in selectors.iter().enumerate() {
            if index != 0 {
                bytes.push(b',');
            }
            crate::summary::encode_text(selector, &mut bytes);
        }
        bytes.push(b']');
        SelectorIdentity {
            schema: SCHEMA.to_owned(),
            count: selectors.len(),
            sha256: format!("{:x}", Sha256::digest(&bytes)),
            prefix: selectors
                .iter()
                .take(3)
                .map(|text| text.prefix(120))
                .collect(),
        }
    }

    /// A malformed identity is not promoted to an identity by ignoring extra fields.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub enum Selection {
        Full(Vec<PythonText>),
        Identity(SelectorIdentity),
        Invalid,
    }

    /// #704: comparing prefixes alone never establishes exact selection equality.
    pub fn selection_matches(candidate: &Selection, selectors: &[PythonText]) -> bool {
        match candidate {
            Selection::Full(full) => full == selectors,
            Selection::Identity(identity) => {
                let expected = selector_identity(selectors);
                identity_fields_match(
                    identity.schema == expected.schema,
                    identity.count == expected.count,
                    identity.sha256 == expected.sha256,
                    identity.prefix == expected.prefix,
                )
            }
            Selection::Invalid => false,
        }
    }
}

#[cfg(not(verus_keep_ghost))]
pub use runtime::{SCHEMA, Selection, SelectorIdentity, selection_matches, selector_identity};

/// #704/#723: a matching display prefix never substitutes for exact identity.
#[cfg_attr(
    verus_keep_ghost,
    vstd::prelude::verus_spec(matches => ensures
        matches == (schema && count && digest && prefix),
        matches ==> digest && count && schema,
    )
)]
pub fn identity_fields_match(schema: bool, count: bool, digest: bool, prefix: bool) -> bool {
    schema && count && digest && prefix
}
