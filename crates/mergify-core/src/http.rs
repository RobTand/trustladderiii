//! GitHub REST status codes to the failure classes looks and retries act on.

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

    pub fn permanent(self) -> bool {
        matches!(
            self,
            Self::Permission | Self::NotFound | Self::InvalidRequest
        )
    }
}

/// 429 is always a rate limit. A 403 is one when GitHub says so: no requests
/// remaining, a `retry-after` header, or a secondary-limit message.
pub fn rate_limited(status: u16, rate_signal: bool) -> bool {
    status == 429 || status == 403 && rate_signal
}

/// The class of an error response that was not a rate limit.
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
pub fn classify(status: u16, rate_signal: bool) -> StatusClass {
    if rate_limited(status, rate_signal) {
        StatusClass::Quota
    } else {
        status_class(status)
    }
}
