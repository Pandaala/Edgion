//! One LDAP exchange (bind, search, StartTLS, unbind) over an
//! already-connected stream.
//!
//! The caller owns dialing, egress admission and TLS. [`BindSession`] writes
//! requests and reads exactly one bounded frame per response.

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::codec::{frame_content_len, length_octets};
use crate::filter::SearchFilter;
use crate::proto::{
    decode_bind_response, decode_extended_response, decode_search_frame, encode_bind_request, encode_search_request,
    encode_start_tls_request, encode_unbind_request, LdapResult, SearchFrame,
};
use crate::Error;
use zeroize::Zeroizing;

/// Maximum continuation references accepted in one search.
pub const MAX_SEARCH_REFERENCES: usize = 8;

/// Search scope; the size limit follows from it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// The whole subtree under the base (sizeLimit 2: enough to see a second match).
    Subtree,
    /// Only the base entry itself (sizeLimit 1).
    Base,
}

/// The single entry a search found.
///
/// `identity` is the entry's single value of the requested attribute, when
/// it has exactly one non-empty, control-free value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundEntry {
    pub dn: String,
    pub identity: Option<String>,
}

/// Longest frame header: identifier, initial length octet, four length octets.
const MAX_FRAME_HEADER: usize = 6;

/// The next LDAP message ID of a session.
///
/// Only [`BindSession::start_tls`] produces one, so callers cannot resume with
/// the reserved unsolicited-notification ID 0 or an out-of-range value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MessageId(u32);

/// Bind, search, unbind and StartTLS over `S`.
///
/// **StartTLS invariant:** a response read consumes exactly the bytes of one
/// frame and never reads ahead (no buffered reader, no speculative read).
/// Otherwise bytes following the StartTLS ExtendedResponse could be consumed
/// before the caller hands the raw stream to the TLS handshake. Pass the raw
/// transport (for example `tokio::net::TcpStream`), never a buffered wrapper
/// such as `tokio::io::BufReader`, whose prefetch would break the same invariant.
pub struct BindSession<S> {
    stream: S,
    next_id: u32,
    buf: Vec<u8>,
}

impl<S: AsyncRead + AsyncWrite + Unpin> BindSession<S> {
    /// Start a session whose first request carries message ID 1.
    pub fn new(stream: S) -> Self {
        Self::resume(stream, MessageId(1))
    }

    /// Continue the message ID sequence on a new stream, typically the TLS
    /// stream returned after [`BindSession::start_tls`].
    pub fn resume(stream: S, next_id: MessageId) -> Self {
        Self {
            stream,
            next_id: next_id.0,
            buf: Vec::new(),
        }
    }

    /// Send a simple BindRequest and decode its BindResponse.
    pub async fn simple_bind(&mut self, bind_dn: &str, password: &[u8]) -> Result<LdapResult, Error> {
        let id = self.send(|id| encode_bind_request(id, bind_dn, password)).await?;
        self.read_frame().await?;
        decode_bind_response(&self.buf, id)
    }

    /// Request StartTLS. On success returns the raw stream, positioned right
    /// after the ExtendedResponse, and the next message ID for [`BindSession::resume`].
    pub async fn start_tls(mut self) -> Result<(S, MessageId), Error> {
        let id = self.send(|id| Zeroizing::new(encode_start_tls_request(id))).await?;
        self.read_frame().await?;
        let result = decode_extended_response(&self.buf, id)?;
        if result.result_code != 0 {
            return Err(Error::StartTlsRejected(result.result_code));
        }
        Ok((self.stream, MessageId(self.next_id)))
    }

    /// Search `base` with `scope` and `filter`, inserting `username` as raw
    /// assertion bytes (ignored by a filter without a slot), and return the
    /// entry only when exactly one entry matched and the search completed with
    /// success. `attribute` is the one attribute requested (`1.1` requests
    /// none).
    ///
    /// Returns `Ok(None)` for no entry, a second entry, or sizeLimitExceeded;
    /// callers must not tell these apart. After a second entry the method
    /// returns without reading the rest of the response, so the session
    /// holds unread frames: only `unbind` and drop it afterwards.
    pub async fn search_unique(
        &mut self,
        base: &str,
        scope: Scope,
        filter: &SearchFilter,
        username: &[u8],
        attribute: &str,
        time_limit_secs: u32,
    ) -> Result<Option<FoundEntry>, Error> {
        const SIZE_LIMIT_EXCEEDED: u32 = 4;
        let id = self
            .send(|id| {
                Zeroizing::new(encode_search_request(
                    id,
                    base,
                    scope,
                    filter,
                    username,
                    attribute,
                    time_limit_secs,
                ))
            })
            .await?;
        let mut found: Option<FoundEntry> = None;
        let mut references = 0usize;
        loop {
            self.read_frame().await?;
            match decode_search_frame(&self.buf, id, attribute)? {
                SearchFrame::Entry { dn, identity } => {
                    if dn.is_empty() {
                        return Err(Error::EmptyDn);
                    }
                    if found.is_some() {
                        return Ok(None);
                    }
                    found = Some(FoundEntry { dn, identity });
                }
                SearchFrame::Reference => {
                    references += 1;
                    if references > MAX_SEARCH_REFERENCES {
                        return Err(Error::TooManyReferences);
                    }
                }
                SearchFrame::Done(result) => {
                    return match result.result_code {
                        0 => Ok(found),
                        SIZE_LIMIT_EXCEEDED => Ok(None),
                        code => Err(Error::SearchFailed(code)),
                    };
                }
            }
        }
    }

    /// Send an UnbindRequest, then shut down the write side (on a TLS stream
    /// this sends close_notify). The protocol defines no response.
    pub async fn unbind(&mut self) -> Result<(), Error> {
        self.send(|id| Zeroizing::new(encode_unbind_request(id))).await?;
        self.stream.shutdown().await.map_err(io_error)
    }

    /// Write one request. Every request buffer is zeroized on drop (also on a
    /// write error or a cancelled future); the type makes callers wrap it, so a
    /// bind request can never be sent from an unwiped buffer.
    async fn send(&mut self, encode: impl FnOnce(u32) -> Zeroizing<Vec<u8>>) -> Result<u32, Error> {
        let id = self.next_id;
        self.next_id = id.saturating_add(1);
        let request = encode(id);
        self.stream.write_all(&request).await.map_err(io_error)?;
        self.stream.flush().await.map_err(io_error)?;
        Ok(id)
    }

    /// Read exactly one frame and leave its content in `self.buf`.
    async fn read_frame(&mut self) -> Result<(), Error> {
        let mut header = [0u8; MAX_FRAME_HEADER];
        let (prefix, rest) = header.split_at_mut(2);
        self.stream.read_exact(prefix).await.map_err(io_error)?;
        let extra = length_octets(prefix[0], prefix[1])?;
        let extra_octets = rest.get_mut(..extra).ok_or(Error::InvalidLength)?;
        self.stream.read_exact(extra_octets).await.map_err(io_error)?;
        let len = frame_content_len(prefix[1], extra_octets)?;
        self.buf.clear();
        self.buf.resize(len, 0);
        self.stream.read_exact(&mut self.buf).await.map_err(io_error)?;
        Ok(())
    }
}

fn io_error(err: std::io::Error) -> Error {
    if err.kind() == std::io::ErrorKind::UnexpectedEof {
        Error::Closed
    } else {
        Error::Io(err)
    }
}

#[cfg(test)]
#[path = "../tests/unit/client_test.rs"]
mod tests;
