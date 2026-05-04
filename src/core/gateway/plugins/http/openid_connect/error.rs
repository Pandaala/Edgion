//! OIDC plugin verification error type.
//!
//! 仅为实际驱动控制流分支的错误语义建模专用变体；其余错误用 `Other` 兜底以保留
//! HTTP status 与用户可见消息的精确还原。

use std::borrow::Cow;

use jsonwebtoken::errors::ErrorKind;

use crate::core::common::jwt_common::map_jwt_decode_error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum OidcVerifyError {
    Expired,
    InvalidFormat,
    RefreshSingleflightTimeout,
    JwksFetch(String),
    Other { status: u16, message: String },
}

impl OidcVerifyError {
    pub(super) fn http_status(&self) -> u16 {
        match self {
            OidcVerifyError::Expired | OidcVerifyError::InvalidFormat => 401,
            OidcVerifyError::RefreshSingleflightTimeout | OidcVerifyError::JwksFetch(_) => 502,
            OidcVerifyError::Other { status, .. } => *status,
        }
    }

    pub(super) fn user_message(&self) -> Cow<'_, str> {
        match self {
            OidcVerifyError::Expired => Cow::Borrowed("Token expired"),
            OidcVerifyError::InvalidFormat => Cow::Borrowed("Invalid token format"),
            OidcVerifyError::RefreshSingleflightTimeout => Cow::Borrowed("Token refresh singleflight wait timeout"),
            OidcVerifyError::JwksFetch(detail) => Cow::Owned(format!("Failed to fetch JWKS: {}", detail)),
            OidcVerifyError::Other { message, .. } => Cow::Borrowed(message.as_str()),
        }
    }

    pub(super) fn log_tag(&self) -> &'static str {
        match self {
            OidcVerifyError::Expired => "token verify fail",
            OidcVerifyError::InvalidFormat => "token decode fail",
            OidcVerifyError::RefreshSingleflightTimeout => "token refresh fail",
            OidcVerifyError::JwksFetch(_) => "jwks fetch fail",
            OidcVerifyError::Other { status, .. } => {
                if *status == 502 {
                    "introspect fail"
                } else {
                    "token verify fail"
                }
            }
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
            "Failed to fetch JWKS: connection refused"
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
