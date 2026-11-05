#[path = "../src/status.rs"]
mod status;
use status::*;
use vstd::prelude::*;
verus! {
/// #735/#743: all 131,072 inputs, preserving each original Kani assertion.
fn http_status_class_total(code: u16, signal: bool) {
    let class = classify(code, signal);
    assert(class == expected(code, signal));
    let limited = rate_limited(code, signal);
    assert(limited == (class == StatusClass::Quota));
    let plain = status_class(code);
    assert(plain != StatusClass::Quota);
}
/// #735/#743: six original reachability covers execute the real classifier.
fn status_witness(case: u8) -> (result: bool)
    requires case < 6,
    ensures result,
{
    let code = match case { 0 | 1 => 403, 2 => 410, 3 => 422, 4 => 500, _ => 400 };
    let class = classify(code, case == 0);
    match case {
        0 => class == StatusClass::Quota && code == 403,
        1 => class == StatusClass::Permission && code == 403,
        2 => class == StatusClass::NotFound && code == 410,
        3 => class == StatusClass::InvalidRequest,
        4 => class == StatusClass::UpstreamServer,
        _ => class == StatusClass::Transport && code == 400,
    }
}
}
fn main() {}
