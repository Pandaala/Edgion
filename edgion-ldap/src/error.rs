//! Error type shared by the codec, protocol and session layers.
//!
//! `Display` and `Debug` render fixed text plus integers; no peer-controlled
//! bytes and no source error text are retained (`Io` keeps only the kind).

/// Failure of an LDAP exchange or of decoding one LDAP message.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The peer closed the stream before a complete message arrived.
    #[error("ldap stream closed by peer")]
    Closed,
    /// Reading from or writing to the stream failed; only the kind is kept.
    #[error("ldap stream i/o error")]
    Io(std::io::ErrorKind),
    /// The frame header declares more content than the fixed `MAX_FRAME`.
    #[error("ldap frame declares {declared} bytes, maximum {max}")]
    FrameTooLarge { declared: usize, max: usize },
    /// An element header or content extends past its container.
    #[error("ldap element extends past its container")]
    Truncated,
    /// Indefinite length, or a length wider than four octets.
    #[error("ldap length encoding is indefinite or wider than four octets")]
    InvalidLength,
    /// High-tag-number form identifiers are not used by LDAP.
    #[error("ldap multi-byte tag numbers are not supported")]
    UnsupportedTag,
    /// An element carries a different identifier than the protocol requires.
    #[error("ldap unexpected tag 0x{actual:02x}, expected 0x{expected:02x}")]
    UnexpectedTag { expected: u8, actual: u8 },
    /// An INTEGER or ENUMERATED is empty, negative, or wider than 32 bits.
    #[error("ldap integer is empty, negative or wider than 32 bits")]
    InvalidInteger,
    /// An LDAPString is not valid UTF-8.
    #[error("ldap string is not valid UTF-8")]
    InvalidUtf8,
    /// The response does not answer the outstanding request.
    #[error("ldap response message id {actual}, expected {expected}")]
    UnexpectedMessageId { expected: u32, actual: u32 },
    /// The server answered the StartTLS request with a non-success code.
    #[error("ldap StartTLS rejected with result code {0}")]
    StartTlsRejected(u32),
    /// The search finished with a result code other than success or sizeLimitExceeded.
    #[error("ldap search failed with result code {0}")]
    SearchFailed(u32),
    /// The search returned more continuation references than allowed.
    #[error("ldap search returned too many references")]
    TooManyReferences,
    /// A search entry carried an empty DN.
    #[error("ldap search entry has an empty DN")]
    EmptyDn,
    /// A simple bind with an empty DN or password: anonymous or unauthenticated
    /// (RFC 4513 section 5.1), so it proves nothing. Refused before any I/O.
    #[error("ldap simple bind requires a non-empty DN and password")]
    UnauthenticatedBind,
    /// A search filter with a `{username}` slot was given an empty username.
    /// Refused before any I/O.
    #[error("ldap search filter requires a non-empty username")]
    EmptyUsername,
    /// An earlier exchange failed, was cancelled, or left unread frames, so
    /// the stream may be out of sync. Only `unbind` remains (shutdown only).
    #[error("ldap session is unusable after an interrupted or failed exchange")]
    SessionBroken,
    /// The session used every message ID up to the RFC 4511 `maxInt`.
    #[error("ldap session exhausted its message IDs")]
    MessageIdExhausted,
}
