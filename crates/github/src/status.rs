//! GitHub REST status codes to the failure classes looks and retries act on
//! (#735). Pure, with a Verus proof; `Error::failure_class` and the rate-limit
//! checks in `client` use it.

#[cfg(verus_keep_ghost)]
use vstd::prelude::*;
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_verify)]
#[cfg_attr(verus_keep_ghost, derive(vstd::prelude::Structural))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusClass {
    /// A rate limit: wait for the reset, spend no retry.
    Quota,
    /// 401 or 403: the app lacks access. Permanent.
    Permission,
    /// 404 or 410: the object is gone. Permanent.
    NotFound,
    /// 422: GitHub refused the request itself. Permanent.
    InvalidRequest,
    /// 5xx: retry later.
    UpstreamServer,
    /// Any other status: retry later.
    Transport,
}

impl StatusClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Quota => "quota",
            Self::Permission => "permission",
            Self::NotFound => "not_found",
            Self::InvalidRequest => "invalid_request",
            Self::UpstreamServer => "upstream_server",
            Self::Transport => "transport",
        }
    }

    #[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result => ensures
    result == (self == StatusClass::Permission || self == StatusClass::NotFound || self == StatusClass::InvalidRequest),
))]
    pub fn permanent(self) -> bool {
        matches!(
            self,
            Self::Permission | Self::NotFound | Self::InvalidRequest
        )
    }
}

/// 429 is always a rate limit. A 403 is one when GitHub says so: no requests
/// remaining, a `retry-after` header, or a secondary-limit message.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result => ensures
    result == (status == 429 || status == 403 && rate_signal),
))]
pub fn rate_limited(status: u16, rate_signal: bool) -> bool {
    status == 429 || status == 403 && rate_signal
}

/// The class of an error response that was not a rate limit.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result => ensures
    result == plain_class(status), result != StatusClass::Quota,
))]
pub fn status_class(status: u16) -> StatusClass {
    match status {
        401 | 403 => StatusClass::Permission,
        404 | 410 => StatusClass::NotFound,
        422 => StatusClass::InvalidRequest,
        500..=599 => StatusClass::UpstreamServer,
        _ => StatusClass::Transport,
    }
}

/// The class of any error response.
#[cfg_attr(verus_keep_ghost, vstd::prelude::verus_spec(result => ensures
    result == expected(status, rate_signal), (result == StatusClass::Quota) == (status == 429 || status == 403 && rate_signal),
))]
pub fn classify(status: u16, rate_signal: bool) -> StatusClass {
    if rate_limited(status, rate_signal) {
        StatusClass::Quota
    } else {
        status_class(status)
    }
}

#[cfg(verus_keep_ghost)]
verus! {
/// #735/#743: plain HTTP status table never yields a rate-limit classification.
pub open spec fn plain_class(status: u16) -> StatusClass {
    match status {
        401 | 403 => StatusClass::Permission,
        404 | 410 => StatusClass::NotFound,
        422 => StatusClass::InvalidRequest,
        500..=599 => StatusClass::UpstreamServer,
        _ => StatusClass::Transport,
    }
}
/// #735/#743: full Kani oracle, preserving the 403 rate-signal distinction.
pub open spec fn expected(status: u16, signal: bool) -> StatusClass {
    match status {
        429 => StatusClass::Quota,
        403 if signal => StatusClass::Quota,
        _ => plain_class(status),
    }
}
}
/// Kani proof that the table is total and exact (#735). A `u16` and a flag
/// have no loops, so unwind 1 bounds the proof; it covers all 131,072 inputs.
#[cfg(kani)]
mod proofs {
    use super::*;

    /// http_status_class_total: every status code, with or without GitHub's
    /// rate-limit signal, has exactly one class, as the table states.
    fn check_http_status_class_total() {
        let status: u16 = kani::any();
        let signal: bool = kani::any();
        let class = classify(status, signal);
        let expected = match status {
            429 => StatusClass::Quota,
            403 if signal => StatusClass::Quota,
            401 | 403 => StatusClass::Permission,
            404 | 410 => StatusClass::NotFound,
            422 => StatusClass::InvalidRequest,
            500..=599 => StatusClass::UpstreamServer,
            _ => StatusClass::Transport,
        };
        assert!(class == expected, "status misclassified");
        assert!(
            rate_limited(status, signal) == (class == StatusClass::Quota),
            "rate limit and quota disagree"
        );
        assert!(
            status_class(status) != StatusClass::Quota,
            "a plain error response is quota"
        );
        kani::cover!(
            class == StatusClass::Quota && status == 403,
            "a 403 rate limit"
        );
        kani::cover!(
            class == StatusClass::Permission && status == 403,
            "a 403 permission"
        );
        kani::cover!(class == StatusClass::NotFound && status == 410, "gone");
        kani::cover!(class == StatusClass::InvalidRequest, "invalid request");
        kani::cover!(class == StatusClass::UpstreamServer, "server error");
        kani::cover!(
            class == StatusClass::Transport && status == 400,
            "another status"
        );
    }

    #[kani::proof]
    #[kani::unwind(1)]
    fn http_status_class_total() {
        check_http_status_class_total();
    }
}
