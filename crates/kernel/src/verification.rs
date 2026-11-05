// Both builds compile the same executable body. Ordinary Rust erases only the
// contracts, while the narrow Verus root checks them against that body.
#[cfg(not(verus_keep_ghost))]
macro_rules! kernel_snapshot {
    ($name:ident = $($value:tt)*) => {};
}

#[cfg(verus_keep_ghost)]
macro_rules! kernel_snapshot {
    ($name:ident = $($value:tt)*) => {
        let $name = vstd::prelude::verus_exec_expr! { Ghost($($value)*) };
    };
}

#[cfg(not(verus_keep_ghost))]
macro_rules! kernel_loop {
    ([$($contract:tt)*] $body:block) => {
        loop $body
    };
}

#[cfg(verus_keep_ghost)]
macro_rules! kernel_loop {
    ([$($contract:tt)*] $body:block) => {
        vstd::prelude::verus_exec_expr! {
            loop $($contract)* $body
        }
    };
}

#[cfg(not(verus_keep_ghost))]
macro_rules! kernel_while {
    (($condition:expr) [$($contract:tt)*] $body:block) => {
        while $condition $body
    };
}

#[cfg(verus_keep_ghost)]
macro_rules! kernel_while {
    (($condition:expr) [$($contract:tt)*] $body:block) => {
        vstd::prelude::verus_exec_expr! {
            while $condition $($contract)* $body
        }
    };
}

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
    (impl $self_type:ty { $(#[$attribute:meta])* $visibility:vis const fn $name:ident $arguments:tt
        -> ($result:ident: $return_type:ty) [$($contract:tt)*] $body:block }) => {
        impl $self_type {
            $(#[$attribute])* $visibility const fn $name $arguments -> $return_type $body
        }
    };
    (impl $self_type:ty { $(#[$attribute:meta])* $visibility:vis fn $name:ident $arguments:tt
        -> ($result:ident: $return_type:ty) [$($contract:tt)*] $(proof [$($proof:tt)*])? $body:block }) => {
        impl $self_type {
            $(#[$attribute])* $visibility fn $name $arguments -> $return_type $body
        }
    };
    ($(#[$attribute:meta])* $visibility:vis fn $name:ident $arguments:tt
        -> ($result:ident: $return_type:ty) [$($contract:tt)*] $(proof [$($proof:tt)*])? $body:block) => {
        $(#[$attribute])* $visibility fn $name $arguments -> $return_type $body
    };
}

#[cfg(verus_keep_ghost)]
macro_rules! kernel_fn {
    (impl $self_type:ty { $(#[$attribute:meta])* $visibility:vis const fn $name:ident $arguments:tt
        -> ($result:ident: $return_type:ty) [$($contract:tt)*] $body:block }) => {
        vstd::prelude::verus! {
            impl $self_type {
                $(#[$attribute])* $visibility fn $name $arguments
                    -> ($result: $return_type) $($contract)* $body
            }
        }
    };
    // Verus must see the impl when expanding a static associated function.
    (impl $self_type:ty { $(#[$attribute:meta])* $visibility:vis fn $name:ident $arguments:tt
        -> ($result:ident: $return_type:ty) [$($contract:tt)*] $(proof [$($proof:tt)*])? $body:block }) => {
        vstd::prelude::verus! {
            impl $self_type {
                $(#[$attribute])* $visibility fn $name $arguments
                    -> ($result: $return_type) $($contract)* {
                        $(proof { $($proof)* })?
                        $body
                    }
            }
        }
    };
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
