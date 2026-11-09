//! Bounded LDAP client subset for Edgion's `LdapAuth` plugin.
//!
//! The crate covers exactly what a simple-bind authenticator needs: the
//! BindRequest/BindResponse exchange, UnbindRequest, and the StartTLS extended
//! operation (RFC 4511, RFC 4513). It replaces the third-party `ldap3`/`lber`
//! stack, whose decoder panicked on malformed replies and recursed once per
//! nested BER element (findings auth-01 and auth-02).
//!
//! Design rules that must keep holding:
//!
//! - **No recursion.** No function in this crate is directly or indirectly
//!   recursive. Every decoder is straight-line code of static depth over the
//!   flat [`codec`] cursor, so no peer byte sequence can grow the stack.
//! - **Bounded frames.** An inbound LDAPMessage is limited to
//!   [`codec::MAX_FRAME`] bytes; the limit is checked from the at most six
//!   header bytes, before any allocation or body read.
//! - **No peer text is retained.** Result strings (`matchedDN`,
//!   `diagnosticMessage`) are validated as UTF-8 and discarded; only the
//!   result code leaves the decoder.
//! - **Sans-IO core.** [`codec`] and [`proto`] never touch I/O and are the
//!   fuzzing entry points; [`client::BindSession`] adds the stream handling.
//!
//! The crate is runtime-light rather than runtime-free: it depends on tokio's
//! `AsyncRead`/`AsyncWrite` traits only. Callers dial, apply egress policy, and
//! perform TLS handshakes themselves.

#![forbid(unsafe_code)]

pub mod client;
pub mod codec;
pub mod dn;
mod error;
pub mod proto;

pub use client::BindSession;
pub use error::Error;
