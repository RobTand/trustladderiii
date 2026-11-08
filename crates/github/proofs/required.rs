#[macro_use]
#[path = "../src/checks/verification.rs"]
mod verification;
#[path = "../src/checks/check.rs"]
mod check;
use check::{App, CheckView, Conclusion, Head, Status};
#[path = "../src/checks/required.rs"]
mod required;
mod required_spec;
fn main() {}
