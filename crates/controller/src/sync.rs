#[cfg(loom)]
pub(crate) use loom::{
    sync::{Arc, Mutex, mpsc},
    thread,
};
#[cfg(not(loom))]
pub(crate) use std::{
    sync::{Arc, Mutex, mpsc},
    thread,
};
