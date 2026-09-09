//! OIDC plugin verification error type.
//!
//! Only model dedicated variants for the error semantics that actually drive
//! control-flow branches; fall back to `Other` for the rest, to preserve the
//! exact HTTP status and user-visible message.

use std::borrow::Cow;

use jsonwebtoken::errors::ErrorKind;

use super::jwt_common::map_jwt_decode_error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AuthResponseFailure {
    TooLarge,
    Read,
    Json,
}

impl AuthResponseFailure {
    fn log_tag(self) -> &'static str {
        match self {
            Self::TooLarge => "auth response too large",
            Self::Read => "auth response read fail",
            Self::Json => "auth response json fail",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum OidcVerifyError {
    Expired,
    InvalidFormat,
    RefreshSingleflightTimeout,
    JwksFetch(String),
    AuthResponse(AuthResponseFailure),
    InvalidFormatAfterIntrospection(AuthResponseFailure),
    Other { status: u16, message: String },
}

impl OidcVerifyError {
    pub(super) fn http_status(&self) -> u16 {
        match self {
            OidcVerifyError::Expired
            | OidcVerifyError::InvalidFormat
            | OidcVerifyError::InvalidFormatAfterIntrospection(_) => 401,
            OidcVerifyError::RefreshSingleflightTimeout
            | OidcVerifyError::JwksFetch(_)
            | OidcVerifyError::AuthResponse(_) => 502,
            OidcVerifyError::Other { status, .. } => *status,
        }
    }

    pub(super) fn user_message(&self) -> Cow<'_, str> {
        match self {
            OidcVerifyError::Expired => Cow::Borrowed("Token expired"),
            OidcVerifyError::InvalidFormat | OidcVerifyError::InvalidFormatAfterIntrospection(_) => {
                Cow::Borrowed("Invalid token format")
            }
            OidcVerifyError::RefreshSingleflightTimeout => Cow::Borrowed("Token refresh singleflight wait timeout"),
            OidcVerifyError::JwksFetch(_) | OidcVerifyError::AuthResponse(_) => Cow::Borrowed("Bad Gateway"),
            OidcVerifyError::Other { message, .. } => Cow::Borrowed(message.as_str()),
        }
    }

    pub(super) fn log_tag(&self) -> &'static str {
        match self {
            OidcVerifyError::Expired => "token verify fail",
            OidcVerifyError::InvalidFormat => "token decode fail",
            OidcVerifyError::RefreshSingleflightTimeout => "token refresh fail",
            OidcVerifyError::JwksFetch(_) => "jwks fetch fail",
            OidcVerifyError::AuthResponse(reason) | OidcVerifyError::InvalidFormatAfterIntrospection(reason) => {
                reason.log_tag()
            }
            OidcVerifyError::Other { status, .. } => {
                if *status == 502 {
                    "introspect fail"
                } else {
                    "token verify fail"
                }
            }
        }
    }

    /// Preserve the caller's existing classification for unrelated failures.
    pub(super) fn response_log_tag_or(&self, fallback: &'static str) -> &'static str {
        match self {
            Self::AuthResponse(_) | Self::InvalidFormatAfterIntrospection(_) => self.log_tag(),
            _ => fallback,
        }
    }

    /// Auto mode keeps its original JWT rejection while retaining safe diagnostics.
    pub(super) fn with_introspection_failure(self, failure: Self) -> Self {
        match (self, failure) {
            (Self::InvalidFormat, Self::AuthResponse(reason)) => Self::InvalidFormatAfterIntrospection(reason),
            (jwt_error, _) => jwt_error,
        }
    }
}

pub(super) fn map_jwt_decode_error_typed(err: jsonwebtoken::errors::Error) -> OidcVerifyError {
    match err.kind() {
        ErrorKind::ExpiredSignature => OidcVerifyError::Expired,
        ErrorKind::InvalidToken | ErrorKind::Base64(_) | ErrorKind::Json(_) | ErrorKind::Utf8(_) => {
            OidcVerifyError::InvalidFormat
        }
        _ => {
            let (status, message) = map_jwt_decode_error(err);
            OidcVerifyError::Other { status, message }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_fallback_preserves_wire_status_and_static_response_reason() {
        for reason in [
            AuthResponseFailure::TooLarge,
            AuthResponseFailure::Read,
            AuthResponseFailure::Json,
        ] {
            let error =
                OidcVerifyError::InvalidFormat.with_introspection_failure(OidcVerifyError::AuthResponse(reason));
            assert_eq!(error.http_status(), 401);
            assert_eq!(error.user_message(), "Invalid token format");
            assert_eq!(error.log_tag(), reason.log_tag());
            assert_eq!(error.response_log_tag_or("token verify fail"), reason.log_tag());
        }
        assert_eq!(
            OidcVerifyError::InvalidFormat.with_introspection_failure(OidcVerifyError::Other {
                status: 502,
                message: "existing failure".into()
            }),
            OidcVerifyError::InvalidFormat
        );
    }

    #[test]
    fn http_status_for_typed_variants() {
        assert_eq!(OidcVerifyError::Expired.http_status(), 401);
        assert_eq!(OidcVerifyError::InvalidFormat.http_status(), 401);
        assert_eq!(OidcVerifyError::RefreshSingleflightTimeout.http_status(), 502);
        assert_eq!(OidcVerifyError::JwksFetch("boom".into()).http_status(), 502);
        assert_eq!(
            OidcVerifyError::Other {
                status: 403,
                message: "x".into()
            }
            .http_status(),
            403
        );
    }

    #[test]
    fn user_message_preserves_wire_strings() {
        assert_eq!(OidcVerifyError::Expired.user_message(), "Token expired");
        assert_eq!(OidcVerifyError::InvalidFormat.user_message(), "Invalid token format");
        assert_eq!(
            OidcVerifyError::RefreshSingleflightTimeout.user_message(),
            "Token refresh singleflight wait timeout"
        );
        assert_eq!(
            OidcVerifyError::JwksFetch("connection refused".into()).user_message(),
            "Bad Gateway"
        );
        assert_eq!(
            OidcVerifyError::Other {
                status: 403,
                message: "Insufficient scope".into()
            }
            .user_message(),
            "Insufficient scope"
        );
    }

    #[test]
    fn log_tag_routing_matches_legacy_branches() {
        assert_eq!(OidcVerifyError::JwksFetch("x".into()).log_tag(), "jwks fetch fail");
        assert_eq!(OidcVerifyError::InvalidFormat.log_tag(), "token decode fail");
        assert_eq!(
            OidcVerifyError::RefreshSingleflightTimeout.log_tag(),
            "token refresh fail"
        );
        assert_eq!(
            OidcVerifyError::Other {
                status: 502,
                message: "Introspection request failed".into()
            }
            .log_tag(),
            "introspect fail"
        );
        assert_eq!(
            OidcVerifyError::Other {
                status: 401,
                message: "Inactive token".into()
            }
            .log_tag(),
            "token verify fail"
        );
        assert_eq!(
            OidcVerifyError::Other {
                status: 403,
                message: "Insufficient scope".into()
            }
            .log_tag(),
            "token verify fail"
        );
    }

    #[test]
    fn map_jwt_decode_error_typed_routes_expired_and_format() {
        // ExpiredSignature → Expired
        let expired = jsonwebtoken::errors::Error::from(ErrorKind::ExpiredSignature);
        assert!(matches!(map_jwt_decode_error_typed(expired), OidcVerifyError::Expired));

        // InvalidToken → InvalidFormat
        let invalid = jsonwebtoken::errors::Error::from(ErrorKind::InvalidToken);
        assert!(matches!(
            map_jwt_decode_error_typed(invalid),
            OidcVerifyError::InvalidFormat
        ));

        // InvalidIssuer → Other (non-typed)
        let issuer = jsonwebtoken::errors::Error::from(ErrorKind::InvalidIssuer);
        match map_jwt_decode_error_typed(issuer) {
            OidcVerifyError::Other { status, message } => {
                assert_eq!(status, 401);
                assert_eq!(message, "Invalid token issuer");
            }
            other => panic!("expected Other, got {:?}", other),
        }
    }
}
