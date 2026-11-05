#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

/// Q4 / #718: exact 85% cutoff over every u64 input, with widened products.
#[cfg_attr(
    verus_keep_ghost,
    vstd::prelude::verus_spec(out => ensures
        out == ((tokens as int) * 20 >= (context as int) * 17),
        out == ((tokens as int) * 100 >= (context as int) * 85),
    )
)]
pub fn cutoff(tokens: u64, context: u64) -> bool {
    (tokens as u128) * 100 >= (context as u128) * 85
}
