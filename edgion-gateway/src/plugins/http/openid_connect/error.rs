//! Data-free OIDC failure classifications and their public projection.
use crate::observe::metrics::registry::record_outbound_tls_san_mismatch;
use crate::tls::validation::outbound_san_mismatch_reason;
use crate::util::http_body::stream_body_capped;
use jsonwebtoken::errors::ErrorKind;

/// Bound retained bytes while streaming and discard all provider/parser detail.
pub(super) async fn read_oidc_json<T: serde::de::DeserializeOwned>(
    response: reqwest::Response,
    max_bytes: usize,
) -> Result<T, OidcDependencyFailure> {
    if response
        .content_length()
        .is_some_and(|length| length > max_bytes as u64)
    {
        return Err(OidcDependencyFailure::ResponseTooLarge);
    }
    let (body, truncated) = stream_body_capped(response, max_bytes)
        .await
        .map_err(|_| OidcDependencyFailure::ResponseRead)?;
    if truncated {
        return Err(OidcDependencyFailure::ResponseTooLarge);
    }
    serde_json::from_slice(&body).map_err(|_| OidcDependencyFailure::ResponseJson)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum OidcOperation {
    Discovery,
    Jwks,
    Authorization,
    TokenExchange,
    Refresh,
    Introspection,
    Userinfo,
    Revocation,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum OidcDependencyFailure {
    InvalidEndpoint,
    MissingEndpoint,
    DestinationRejected,
    Timeout,
    Transport,
    TlsSanMismatch,
    HttpStatus,
    ResponseTooLarge,
    ResponseRead,
    ResponseJson,
    InvalidDocument,
    RefreshThrottled,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct OidcDependencyError {
    pub(super) operation: OidcOperation,
    pub(super) failure: OidcDependencyFailure,
}
impl OidcDependencyError {
    pub(super) const fn new(operation: OidcOperation, failure: OidcDependencyFailure) -> Self {
        Self { operation, failure }
    }
    /// Classify before dropping the transport error, which may contain a URL.
    pub(super) fn transport(operation: OidcOperation, error: &reqwest::Error, component: &'static str) -> Self {
        let failure = if outbound_san_mismatch_reason(error).is_some() {
            record_outbound_tls_san_mismatch(component);
            OidcDependencyFailure::TlsSanMismatch
        } else if error.is_timeout() {
            OidcDependencyFailure::Timeout
        } else {
            OidcDependencyFailure::Transport
        };
        Self::new(operation, failure)
    }
    pub(super) fn log_tag(self) -> &'static str {
        match self.operation {
            OidcOperation::Discovery => match self.failure {
                OidcDependencyFailure::InvalidEndpoint => "discovery endpoint invalid",
                OidcDependencyFailure::MissingEndpoint => "discovery endpoint missing",
                OidcDependencyFailure::DestinationRejected => "discovery destination denied",
                OidcDependencyFailure::Timeout => "discovery timeout",
                OidcDependencyFailure::Transport => "discovery transport fail",
                OidcDependencyFailure::TlsSanMismatch => "discovery san mismatch",
                OidcDependencyFailure::HttpStatus => "discovery status fail",
                OidcDependencyFailure::ResponseTooLarge => "discovery response too large",
                OidcDependencyFailure::ResponseRead => "discovery response read fail",
                OidcDependencyFailure::ResponseJson => "discovery response json fail",
                OidcDependencyFailure::InvalidDocument => "discovery document invalid",
                OidcDependencyFailure::RefreshThrottled => "discovery refresh throttled",
            },
            OidcOperation::Jwks => match self.failure {
                OidcDependencyFailure::InvalidEndpoint => "jwks endpoint invalid",
                OidcDependencyFailure::MissingEndpoint => "jwks endpoint missing",
                OidcDependencyFailure::DestinationRejected => "jwks destination denied",
                OidcDependencyFailure::Timeout => "jwks timeout",
                OidcDependencyFailure::Transport => "jwks transport fail",
                OidcDependencyFailure::TlsSanMismatch => "jwks san mismatch",
                OidcDependencyFailure::HttpStatus => "jwks status fail",
                OidcDependencyFailure::ResponseTooLarge => "jwks response too large",
                OidcDependencyFailure::ResponseRead => "jwks response read fail",
                OidcDependencyFailure::ResponseJson => "jwks response json fail",
                OidcDependencyFailure::InvalidDocument => "jwks document invalid",
                OidcDependencyFailure::RefreshThrottled => "jwks refresh throttled",
            },
            OidcOperation::Authorization => match self.failure {
                OidcDependencyFailure::InvalidEndpoint => "authorization endpoint invalid",
                OidcDependencyFailure::MissingEndpoint => "authorization endpoint missing",
                OidcDependencyFailure::DestinationRejected => "authorization destination denied",
                OidcDependencyFailure::Timeout => "authorization timeout",
                OidcDependencyFailure::Transport => "authorization transport fail",
                OidcDependencyFailure::TlsSanMismatch => "authorization san mismatch",
                OidcDependencyFailure::HttpStatus => "authorization status fail",
                OidcDependencyFailure::ResponseTooLarge => "authorization response too large",
                OidcDependencyFailure::ResponseRead => "authorization response read fail",
                OidcDependencyFailure::ResponseJson => "authorization response json fail",
                OidcDependencyFailure::InvalidDocument => "authorization document invalid",
                OidcDependencyFailure::RefreshThrottled => "authorization refresh throttled",
            },
            OidcOperation::TokenExchange => match self.failure {
                OidcDependencyFailure::InvalidEndpoint => "exchange endpoint invalid",
                OidcDependencyFailure::MissingEndpoint => "exchange endpoint missing",
                OidcDependencyFailure::DestinationRejected => "exchange destination denied",
                OidcDependencyFailure::Timeout => "exchange timeout",
                OidcDependencyFailure::Transport => "exchange transport fail",
                OidcDependencyFailure::TlsSanMismatch => "exchange san mismatch",
                OidcDependencyFailure::HttpStatus => "exchange status fail",
                OidcDependencyFailure::ResponseTooLarge => "exchange response too large",
                OidcDependencyFailure::ResponseRead => "exchange response read fail",
                OidcDependencyFailure::ResponseJson => "exchange response json fail",
                OidcDependencyFailure::InvalidDocument => "exchange document invalid",
                OidcDependencyFailure::RefreshThrottled => "exchange refresh throttled",
            },
            OidcOperation::Refresh => match self.failure {
                OidcDependencyFailure::InvalidEndpoint => "refresh endpoint invalid",
                OidcDependencyFailure::MissingEndpoint => "refresh endpoint missing",
                OidcDependencyFailure::DestinationRejected => "refresh destination denied",
                OidcDependencyFailure::Timeout => "refresh timeout",
                OidcDependencyFailure::Transport => "refresh transport fail",
                OidcDependencyFailure::TlsSanMismatch => "refresh san mismatch",
                OidcDependencyFailure::HttpStatus => "refresh status fail",
                OidcDependencyFailure::ResponseTooLarge => "refresh response too large",
                OidcDependencyFailure::ResponseRead => "refresh response read fail",
                OidcDependencyFailure::ResponseJson => "refresh response json fail",
                OidcDependencyFailure::InvalidDocument => "refresh document invalid",
                OidcDependencyFailure::RefreshThrottled => "refresh refresh throttled",
            },
            OidcOperation::Introspection => match self.failure {
                OidcDependencyFailure::InvalidEndpoint => "introspect endpoint invalid",
                OidcDependencyFailure::MissingEndpoint => "introspect endpoint missing",
                OidcDependencyFailure::DestinationRejected => "introspect destination denied",
                OidcDependencyFailure::Timeout => "introspect timeout",
                OidcDependencyFailure::Transport => "introspect transport fail",
                OidcDependencyFailure::TlsSanMismatch => "introspect san mismatch",
                OidcDependencyFailure::HttpStatus => "introspect status fail",
                OidcDependencyFailure::ResponseTooLarge => "introspect response too large",
                OidcDependencyFailure::ResponseRead => "introspect response read fail",
                OidcDependencyFailure::ResponseJson => "introspect response json fail",
                OidcDependencyFailure::InvalidDocument => "introspect document invalid",
                OidcDependencyFailure::RefreshThrottled => "introspect refresh throttled",
            },
            OidcOperation::Userinfo => match self.failure {
                OidcDependencyFailure::InvalidEndpoint => "userinfo endpoint invalid",
                OidcDependencyFailure::MissingEndpoint => "userinfo endpoint missing",
                OidcDependencyFailure::DestinationRejected => "userinfo destination denied",
                OidcDependencyFailure::Timeout => "userinfo timeout",
                OidcDependencyFailure::Transport => "userinfo transport fail",
                OidcDependencyFailure::TlsSanMismatch => "userinfo san mismatch",
                OidcDependencyFailure::HttpStatus => "userinfo status fail",
                OidcDependencyFailure::ResponseTooLarge => "userinfo response too large",
                OidcDependencyFailure::ResponseRead => "userinfo response read fail",
                OidcDependencyFailure::ResponseJson => "userinfo response json fail",
                OidcDependencyFailure::InvalidDocument => "userinfo document invalid",
                OidcDependencyFailure::RefreshThrottled => "userinfo refresh throttled",
            },
            OidcOperation::Revocation => match self.failure {
                OidcDependencyFailure::InvalidEndpoint => "revocation endpoint invalid",
                OidcDependencyFailure::MissingEndpoint => "revocation endpoint missing",
                OidcDependencyFailure::DestinationRejected => "revocation destination denied",
                OidcDependencyFailure::Timeout => "revocation timeout",
                OidcDependencyFailure::Transport => "revocation transport fail",
                OidcDependencyFailure::TlsSanMismatch => "revocation san mismatch",
                OidcDependencyFailure::HttpStatus => "revocation status fail",
                OidcDependencyFailure::ResponseTooLarge => "revocation response too large",
                OidcDependencyFailure::ResponseRead => "revocation response read fail",
                OidcDependencyFailure::ResponseJson => "revocation response json fail",
                OidcDependencyFailure::InvalidDocument => "revocation document invalid",
                OidcDependencyFailure::RefreshThrottled => "revocation refresh throttled",
            },
        }
    }
}
impl std::fmt::Display for OidcDependencyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.log_tag())
    }
}
impl std::error::Error for OidcDependencyError {}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AuthFailure {
    Credentials,
    MalformedCookie,
    InvalidCookieSignature,
    InvalidRedirect,
    MissingRedirectHost,
    InsufficientScope,
    NotYetValid,
    InvalidIssuer,
    InvalidAudience,
    InvalidSignature,
    InvalidAlgorithm,
    InvalidToken,
    InactiveToken,
    MissingRefreshToken,
    InvalidNonce,
    InvalidKey,
    MissingKey,
    NoSuitableKey,
}
impl AuthFailure {
    fn public_message(self) -> &'static str {
        match self {
            Self::Credentials => "Unauthorized",
            Self::MalformedCookie => "Malformed OIDC cookie payload",
            Self::InvalidCookieSignature => "Invalid OIDC cookie signature",
            Self::InvalidRedirect => "Invalid redirectUri",
            Self::MissingRedirectHost => "Missing host header for redirect",
            Self::InsufficientScope => "Insufficient scope",
            Self::NotYetValid => "Token not yet valid",
            Self::InvalidIssuer => "Invalid token issuer",
            Self::InvalidAudience => "Invalid token audience",
            Self::InvalidSignature => "Invalid token signature",
            Self::InvalidAlgorithm => "Invalid token algorithm",
            Self::InvalidToken => "Invalid token",
            Self::InactiveToken => "Inactive token",
            Self::MissingRefreshToken => "Missing refresh token",
            Self::InvalidNonce => "Invalid ID token nonce",
            Self::InvalidKey => "Invalid token",
            Self::MissingKey => "No matching JWK found for token kid",
            Self::NoSuitableKey => "No suitable JWK for token algorithm",
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum OidcVerifyError {
    Expired,
    InvalidFormat,
    RefreshSingleflightTimeout,
    Dependency(OidcDependencyError),
    InvalidFormatAfterIntrospection(OidcDependencyError),
    Auth(AuthFailure),
    SessionEncoding,
}
impl OidcVerifyError {
    pub(super) const fn dependency(operation: OidcOperation, failure: OidcDependencyFailure) -> Self {
        Self::Dependency(OidcDependencyError::new(operation, failure))
    }
    pub(super) fn http_status(&self) -> u16 {
        match self {
            Self::Dependency(_) | Self::RefreshSingleflightTimeout | Self::SessionEncoding => 502,
            Self::Auth(AuthFailure::InsufficientScope) => 403,
            _ => 401,
        }
    }
    pub(super) fn user_message(&self) -> &'static str {
        match self {
            Self::Expired => "Token expired",
            Self::InvalidFormat | Self::InvalidFormatAfterIntrospection(_) => "Invalid token format",
            Self::Dependency(_) | Self::RefreshSingleflightTimeout | Self::SessionEncoding => "Bad Gateway",
            Self::Auth(reason) => reason.public_message(),
        }
    }
    pub(super) fn log_tag(&self) -> &'static str {
        match self {
            Self::Dependency(reason) | Self::InvalidFormatAfterIntrospection(reason) => reason.log_tag(),
            Self::RefreshSingleflightTimeout => "refresh wait timeout",
            Self::InvalidFormat => "token decode fail",
            Self::SessionEncoding => "session encode fail",
            Self::Auth(AuthFailure::InvalidKey) => "jwks key invalid",
            _ => "token verify fail",
        }
    }
    pub(super) fn response_log_tag_or(&self, fallback: &'static str) -> &'static str {
        match self {
            Self::Dependency(_)
            | Self::InvalidFormatAfterIntrospection(_)
            | Self::RefreshSingleflightTimeout
            | Self::Auth(AuthFailure::InvalidKey) => self.log_tag(),
            _ => fallback,
        }
    }
    pub(super) fn with_introspection_failure(self, failure: Self) -> Self {
        match (self, failure) {
            (Self::InvalidFormat, Self::Dependency(reason)) => Self::InvalidFormatAfterIntrospection(reason),
            (jwt_error, _) => jwt_error,
        }
    }
}
impl From<OidcDependencyError> for OidcVerifyError {
    fn from(error: OidcDependencyError) -> Self {
        Self::Dependency(error)
    }
}
pub(super) fn map_jwt_decode_error_typed(err: jsonwebtoken::errors::Error) -> OidcVerifyError {
    match err.kind() {
        ErrorKind::ExpiredSignature => OidcVerifyError::Expired,
        ErrorKind::InvalidToken | ErrorKind::Base64(_) | ErrorKind::Json(_) | ErrorKind::Utf8(_) => {
            OidcVerifyError::InvalidFormat
        }
        ErrorKind::InvalidIssuer => OidcVerifyError::Auth(AuthFailure::InvalidIssuer),
        ErrorKind::InvalidAudience => OidcVerifyError::Auth(AuthFailure::InvalidAudience),
        ErrorKind::ImmatureSignature => OidcVerifyError::Auth(AuthFailure::NotYetValid),
        ErrorKind::InvalidSignature => OidcVerifyError::Auth(AuthFailure::InvalidSignature),
        ErrorKind::InvalidAlgorithm => OidcVerifyError::Auth(AuthFailure::InvalidAlgorithm),
        _ => OidcVerifyError::Auth(AuthFailure::InvalidToken),
    }
}
