#![forbid(unsafe_code)]

mod client;
pub mod message;
mod models;
pub mod webhook;

pub use client::{API_VERSION, AppConfig, ClientOptions, Error, FailureClass, GitHub};
pub use models::*;

pub fn closing_issues(_body: &str) -> Vec<u64> {
    todo!("Q45 closing keywords")
}
