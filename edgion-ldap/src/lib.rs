//! Bounded LDAP client subset for Edgion's `LdapAuth` plugin.
//!
//! The crate covers exactly what a simple-bind authenticator needs: the
//! BindRequest/BindResponse exchange, UnbindRequest, the StartTLS extended
//! operation (RFC 4511, RFC 4513), and for search mode a single-entry
//! SearchRequest with an injection-proof RFC 4515 filter compiler
//! ([`filter`]) and DN comparison ([`dn`]). It replaces the third-party `ldap3`/`lber`
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
//! - **Minimal peer text is retained.** Result strings (`matchedDN`,
//!   `diagnosticMessage`) are validated as UTF-8 and discarded. Only result
//!   codes leave the decoder, plus, in search mode, one matched entry DN and
//!   one identity value; both are bounded by [`codec::MAX_FRAME`] and callers
//!   must never log them or put them in a response.
//! - **Sans-IO core.** [`codec`], [`proto`], [`filter`] and [`dn`] never touch
//!   I/O and are the fuzzing entry points; the `client` feature adds
//!   `client::BindSession` for the stream handling.
//!
//! With the default `client` feature the crate is runtime-light rather than
//! runtime-free: it depends on tokio's `AsyncRead`/`AsyncWrite` traits and
//! on `zeroize` (to wipe request buffers that carry a bind password).
//! Without it the crate has no runtime dependency. Callers dial, apply egress
//! policy, and perform TLS handshakes themselves.

#![forbid(unsafe_code)]

#[cfg(feature = "client")]
pub mod client;
pub mod codec;
pub mod dn;
mod error;
pub mod filter;
pub mod proto;

#[cfg(feature = "client")]
pub use client::{BindSession, FoundEntry, MessageId};
pub use error::Error;
pub use proto::Scope;
