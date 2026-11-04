// Both builds compile the same executable body. Ordinary Rust erases only the
// contracts, while the narrow Verus root checks them against that body.
#[cfg(not(verus_keep_ghost))]
macro_rules! kernel_proof {
    ($($proof:tt)*) => {};
}

#[cfg(verus_keep_ghost)]
macro_rules! kernel_proof {
    ($($proof:tt)*) => {
        vstd::prelude::verus_exec_expr! { proof { $($proof)* } }
    };
}

#[cfg(not(verus_keep_ghost))]
macro_rules! kernel_fn {
    ($(#[$attribute:meta])* $visibility:vis fn $name:ident $arguments:tt
        -> ($result:ident: $return_type:ty) [$($contract:tt)*] $(proof [$($proof:tt)*])? $body:block) => {
        $(#[$attribute])* $visibility fn $name $arguments -> $return_type $body
    };
}

#[cfg(verus_keep_ghost)]
macro_rules! kernel_fn {
    ($(#[$attribute:meta])* $visibility:vis fn $name:ident $arguments:tt
        -> ($result:ident: $return_type:ty) [$($contract:tt)*] $(proof [$($proof:tt)*])? $body:block) => {
        vstd::prelude::verus! {
            $(#[$attribute])* $visibility fn $name $arguments
                -> ($result: $return_type) $($contract)* {
                    $(proof { $($proof)* })?
                    $body
                }
        }
    };
}
