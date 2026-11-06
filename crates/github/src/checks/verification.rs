// #743: Rust erases contracts; Verus checks the same executable bodies.
#[cfg(not(verus_keep_ghost))]
macro_rules! decide_while {
    (($condition:expr) [$($contract:tt)*] $body:block) => {
        while $condition $body
    };
}

#[cfg(verus_keep_ghost)]
macro_rules! decide_while {
    (($condition:expr) [$($contract:tt)*] $body:block) => {
        vstd::prelude::verus_exec_expr! {
            while $condition $($contract)* $body
        }
    };
}

#[cfg(not(verus_keep_ghost))]
macro_rules! decide_proof {
    ($($proof:tt)*) => {};
}
#[cfg(verus_keep_ghost)]
macro_rules! decide_proof {
    ($($proof:tt)*) => { vstd::prelude::verus_exec_expr! { proof { $($proof)* } } };
}
#[cfg(not(verus_keep_ghost))]
macro_rules! decide_predicate {
    (|$a:ident| $body:expr) => {
        |$a: &CheckView| $body
    };
    (|$a:ident, $b:ident| $body:expr) => {
        |$a: &CheckView, $b: &CheckView| $body
    };
}
#[cfg(verus_keep_ghost)]
macro_rules! decide_predicate {
    (|$a:ident| $body:expr) => {
        vstd::prelude::verus_exec_expr! {
            |$a: &CheckView| -> (result: bool) ensures result == ($body) { $body }
        }
    };
    (|$a:ident, $b:ident| $body:expr) => {
        vstd::prelude::verus_exec_expr! {
            |$a: &CheckView, $b: &CheckView| -> (result: bool) ensures result == ($body) { $body }
        }
    };
}
#[cfg(all(not(verus_keep_ghost), test))]
macro_rules! decide_report {
    ($ty:ty; $body:expr) => {
        || $body
    };
    (unread $ty:ty; $body:expr) => {
        || $body
    };
}
#[cfg(verus_keep_ghost)]
macro_rules! decide_report {
    (unread $ty:ty; $body:expr) => {
        vstd::prelude::verus_exec_expr! {
            || -> (result: $ty) requires false ensures result == ($body) { $body }
        }
    };
    ($ty:ty; $body:expr) => {
        vstd::prelude::verus_exec_expr! {
            || -> (result: $ty) ensures result == ($body) { $body }
        }
    };
}

#[cfg(all(not(verus_keep_ghost), test))]
macro_rules! decide_infra {
    (unread $body:expr) => {
        |_| $body
    };
    ($body:expr) => {
        |_| $body
    };
}
#[cfg(verus_keep_ghost)]
macro_rules! decide_infra {
    (unread $body:expr) => {
        vstd::prelude::verus_exec_expr! {
            |check: u8| -> (result: bool) requires false ensures result == ($body) { $body }
        }
    };
    ($body:expr) => {
        vstd::prelude::verus_exec_expr! {
            |check: u8| -> (result: bool) ensures result == ($body) { $body }
        }
    };
}
