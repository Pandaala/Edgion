//! TLS ClientHello SNI extraction without TLS termination.
//!
//! Parses the SNI (Server Name Indication) extension from a raw TLS ClientHello
//! message. Used by TLS Passthrough mode to route connections based on SNI
//! without performing a full TLS handshake.

use edgion_algo::matcher::radix_hostname::normalize_route_sni;

/// Upper bound on bytes drained from the stream while accumulating a
/// ClientHello record.
///
/// Set to 16 KiB — the TLSPlaintext fragment ceiling from RFC 8446 §5.1
/// (`2^14`); this also matches `SSL3_RT_MAX_PLAIN_LENGTH`. The buffer
/// includes the 5-byte record header, so the largest record we can actually
/// admit is `16384 - 5 = 16379` fragment bytes — well above any realistic
/// ClientHello (typically 200-500 bytes, up to a few KiB with many cipher
/// suites, post-quantum key exchange, and numerous extensions).
pub const MAX_CLIENT_HELLO_LEN: usize = 16384;

/// Result of probing whether a buffer already holds a complete TLS Handshake
/// record. Callers use this to decide whether to read more bytes from the
/// downstream before invoking [`extract_sni`].
#[derive(Debug, PartialEq, Eq)]
pub enum RecordCompleteness {
    /// Buffer already contains a full record; `record_end` is the byte offset
    /// just past the record (`buf[..record_end]` is the complete record).
    Complete { record_end: usize },
    /// First byte (if any) is consistent with a TLS Handshake record but the
    /// declared length has not yet arrived. Caller should read more bytes.
    NeedMore,
    /// First byte is present and definitively not a TLS Handshake record
    /// (ContentType != 0x16). Caller should drop the connection.
    NotTlsHandshake,
}

/// Probe whether `buf` already contains a complete TLS Handshake record.
///
/// This is the cheap pre-check called in a read loop: callers accumulate bytes
/// until this returns [`RecordCompleteness::Complete`] (then they invoke
/// [`extract_sni`]) or [`RecordCompleteness::NotTlsHandshake`] (then they
/// close the connection). A returned [`RecordCompleteness::NeedMore`] means
/// the record length is either unknown yet (< 5 bytes seen) or known but more
/// bytes are still in flight.
pub fn client_hello_record_complete(buf: &[u8]) -> RecordCompleteness {
    if buf.is_empty() {
        return RecordCompleteness::NeedMore;
    }
    if buf[0] != 0x16 {
        return RecordCompleteness::NotTlsHandshake;
    }
    if buf.len() < 5 {
        return RecordCompleteness::NeedMore;
    }
    let record_len = u16::from_be_bytes([buf[3], buf[4]]) as usize;
    let record_end = 5 + record_len;
    if buf.len() < record_end {
        return RecordCompleteness::NeedMore;
    }
    RecordCompleteness::Complete { record_end }
}

/// Extract the SNI hostname from a raw TLS ClientHello message buffer.
///
/// The caller is expected to have ensured the buffer holds a complete record
/// (see [`client_hello_record_complete`]). Returns `None` when the buffer is
/// not a valid ClientHello or does not contain an SNI extension; the function
/// never proceeds against a truncated record.
pub fn extract_sni(buf: &[u8]) -> Option<String> {
    // TLS Record Header: ContentType(1) + Version(2) + Length(2)
    if buf.len() < 5 {
        return None;
    }
    if buf[0] != 0x16 {
        // Not a Handshake record
        return None;
    }
    let record_len = u16::from_be_bytes([buf[3], buf[4]]) as usize;
    let record_end = 5 + record_len;
    if buf.len() < record_end {
        // Truncated record — refuse rather than guess at clamped bytes.
        return None;
    }
    let handshake = &buf[5..record_end];

    // Handshake Header: Type(1) + Length(3)
    if handshake.len() < 4 {
        return None;
    }
    if handshake[0] != 0x01 {
        // Not a ClientHello
        return None;
    }
    let hello_len = ((handshake[1] as usize) << 16) | ((handshake[2] as usize) << 8) | handshake[3] as usize;
    // Other handshake messages may follow in this record, but cannot supply
    // bytes to the first ClientHello's declared body.
    let hello = handshake.get(4..4 + hello_len)?;

    // ClientHello body:
    //   ProtocolVersion(2) + Random(32) = 34 bytes fixed
    if hello.len() < 34 {
        return None;
    }
    let mut pos = 34;

    // SessionID: length(1) + data
    if pos >= hello.len() {
        return None;
    }
    let session_id_len = hello[pos] as usize;
    pos += 1 + session_id_len;

    // CipherSuites: length(2) + data
    if pos + 2 > hello.len() {
        return None;
    }
    let cipher_suites_len = u16::from_be_bytes([hello[pos], hello[pos + 1]]) as usize;
    pos += 2 + cipher_suites_len;

    // CompressionMethods: length(1) + data
    if pos >= hello.len() {
        return None;
    }
    let compression_len = hello[pos] as usize;
    pos += 1 + compression_len;

    // Extensions: total_length(2) + extension*
    if pos + 2 > hello.len() {
        return None;
    }
    let extensions_len = u16::from_be_bytes([hello[pos], hello[pos + 1]]) as usize;
    pos += 2;
    let extensions_end = pos + extensions_len;
    if extensions_end != hello.len() {
        return None;
    }
    let mut first_sni = None;

    while pos < extensions_end {
        if pos + 4 > extensions_end {
            return None;
        }
        let ext_type = u16::from_be_bytes([hello[pos], hello[pos + 1]]);
        let ext_len = u16::from_be_bytes([hello[pos + 2], hello[pos + 3]]) as usize;
        pos += 4;
        let data = hello.get(pos..pos + ext_len)?;

        if ext_type == 0x0000 {
            // SNI extension: ServerNameList length(2) + entries
            let hostname = parse_sni_extension(data)?;
            // Keep the existing first-SNI selection, including no host_name.
            // Still check later containers before publishing that selection.
            if first_sni.is_none() {
                first_sni = Some(hostname);
            }
        }

        pos += ext_len;
    }

    first_sni
        .flatten()
        .and_then(|name| std::str::from_utf8(name).ok())
        .and_then(|name| normalize_route_sni(name).ok())
        .map(|name| name.into_owned())
}

/// Validate the complete list, retaining the first host_name without allocating.
/// Outer None is malformed structure; inner None is a list without host_name.
fn parse_sni_extension(data: &[u8]) -> Option<Option<&[u8]>> {
    if data.len() < 2 {
        return None;
    }
    // ServerNameList: list_length(2) + ServerName*
    let list_len = u16::from_be_bytes([data[0], data[1]]) as usize;
    if list_len == 0 || list_len + 2 != data.len() {
        return None;
    }
    let mut pos = 2;
    let mut hostname = None;

    while pos < data.len() {
        if pos + 3 > data.len() {
            return None;
        }
        let name_type = data[pos];
        let name_len = u16::from_be_bytes([data[pos + 1], data[pos + 2]]) as usize;
        pos += 3;
        let name = data.get(pos..pos + name_len)?;

        if name_type == 0x00 && hostname.is_none() {
            // host_name
            hostname = Some(name);
        }

        pos += name_len;
    }

    Some(hostname)
}

#[cfg(test)]
#[path = "../../../tests/unit/routes/tls/sni_parser_test.rs"]
mod tests;
