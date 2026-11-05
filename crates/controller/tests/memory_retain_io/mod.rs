#![allow(dead_code, unused_imports)] // Frozen functions preserve production signatures.
use crate::{Error, Result};
use ::std::{
    io::{BufRead, BufReader, Read},
    path::Path,
    sync::LazyLock,
    time::Duration,
};
use regex::Regex;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tl3_controller_core::memory_retain::{self as retain, Action as MemoryAction};
mod io;
use io::mock_std as std;
use io::{File, Store, reqwest, settings as tl3_config};
include!("types.rs");
include!("helpers.rs");
fn memory_needs_rob(held: &bool, _: &str) -> Result<bool> {
    Ok(*held)
}
pub mod executor {
    use super::*;
    pub fn runtime() -> Result<tokio::runtime::Runtime> {
        io::sample("runtime", json!(null))?;
        Ok(tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap())
    }
}
mod reference {
    include!("reference.rs");
}
mod current {
    include!("current.rs");
}
mod properties;
