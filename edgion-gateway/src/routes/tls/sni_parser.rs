//! TLS ClientHello SNI extraction without TLS termination.
//!
//! Parses the SNI (Server Name Indication) extension from a raw TLS ClientHello
//! message. Used by TLS Passthrough mode to route connections based on SNI
//! without performing a full TLS handshake.

use crate::runtime::matching::hostname_input::normalize_route_sni;

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
mod tests {
    use super::*;

    /// Build a minimal valid TLS 1.2 ClientHello with the given SNI hostname.
    fn build_client_hello(hostname: &str) -> Vec<u8> {
        // SNI extension payload
        let name_bytes = hostname.as_bytes();
        let sni_entry_len = 3 + name_bytes.len(); // name_type(1) + name_len(2) + name
        let sni_list_len = sni_entry_len;
        let sni_ext_data_len = 2 + sni_list_len; // list_length(2) + entries

        let mut sni_ext = Vec::new();
        sni_ext.extend_from_slice(&0x0000u16.to_be_bytes()); // ext type = SNI
        sni_ext.extend_from_slice(&(sni_ext_data_len as u16).to_be_bytes()); // ext length
        sni_ext.extend_from_slice(&(sni_list_len as u16).to_be_bytes()); // list length
        sni_ext.push(0x00); // name type = host_name
        sni_ext.extend_from_slice(&(name_bytes.len() as u16).to_be_bytes());
        sni_ext.extend_from_slice(name_bytes);

        // Add a dummy extension before SNI to test iteration
        let mut extensions = Vec::new();
        // Dummy extension (type=0x0017 "extended_master_secret", len=0)
        extensions.extend_from_slice(&[0x00, 0x17, 0x00, 0x00]);
        extensions.extend_from_slice(&sni_ext);

        // ClientHello body
        let mut hello_body = Vec::new();
        hello_body.extend_from_slice(&[0x03, 0x03]); // version TLS 1.2
        hello_body.extend_from_slice(&[0u8; 32]); // random
        hello_body.push(0); // session_id length = 0
        hello_body.extend_from_slice(&[0x00, 0x02, 0x00, 0x2f]); // cipher suites: 1 suite
        hello_body.extend_from_slice(&[0x01, 0x00]); // compression: 1 method (null)
        hello_body.extend_from_slice(&(extensions.len() as u16).to_be_bytes()); // extensions length
        hello_body.extend_from_slice(&extensions);

        // Handshake header
        let mut handshake = Vec::new();
        handshake.push(0x01); // ClientHello
        let len = hello_body.len();
        handshake.push((len >> 16) as u8);
        handshake.push((len >> 8) as u8);
        handshake.push(len as u8);
        handshake.extend_from_slice(&hello_body);

        // TLS Record header
        let mut record = Vec::new();
        record.push(0x16); // Handshake
        record.extend_from_slice(&[0x03, 0x01]); // TLS 1.0 record version (typical)
        record.extend_from_slice(&(handshake.len() as u16).to_be_bytes());
        record.extend_from_slice(&handshake);

        record
    }

    #[test]
    fn test_extract_sni_basic() {
        let buf = build_client_hello("example.com");
        assert_eq!(extract_sni(&buf), Some("example.com".to_string()));
    }

    // Offsets in build_client_hello's fixed, empty-session-ID fixture.
    const EXTENSIONS_LEN: usize = 50;
    const SNI_EXT: usize = 56;

    fn set_u16(buf: &mut [u8], pos: usize, value: usize) {
        buf[pos..pos + 2].copy_from_slice(&(value as u16).to_be_bytes());
    }

    fn set_handshake_len(buf: &mut [u8], len: usize) {
        buf[6..9].copy_from_slice(&(len as u32).to_be_bytes()[1..]);
    }

    fn replace_extensions(extensions: &[u8]) -> Vec<u8> {
        let mut buf = build_client_hello("example.com");
        buf.truncate(EXTENSIONS_LEN + 2);
        buf.extend_from_slice(extensions);
        set_u16(&mut buf, EXTENSIONS_LEN, extensions.len());
        let len = buf.len();
        set_handshake_len(&mut buf, len - 9);
        set_u16(&mut buf, 3, len - 5);
        buf
    }

    #[test]
    fn test_nested_declared_length_mismatches() {
        let original = build_client_hello("example.com");
        for offset in [EXTENSIONS_LEN, SNI_EXT + 2, SNI_EXT + 4, SNI_EXT + 7] {
            let len = u16::from_be_bytes([original[offset], original[offset + 1]]) as usize;
            for declared in [0, len - 1, len + 1, u16::MAX as usize] {
                let mut buf = original.clone();
                set_u16(&mut buf, offset, declared);
                assert_eq!(extract_sni(&buf), None, "offset={offset}, declared={declared}");
                assert_eq!(
                    client_hello_record_complete(&buf),
                    RecordCompleteness::Complete { record_end: buf.len() }
                );
            }
        }
        for declared in [0, 34, original.len() - 10, original.len() - 8, 0xffffff] {
            let mut buf = original.clone();
            set_handshake_len(&mut buf, declared);
            assert_eq!(extract_sni(&buf), None, "handshake length={declared}");
        }
    }

    #[test]
    fn test_extension_boundaries_before_and_after_sni() {
        let original = build_client_hello("Example.COM");
        let sni = &original[SNI_EXT..];
        let unknown = [0x12, 0x34, 0, 2, 0xaa, 0xbb];
        for before in [true, false] {
            let extensions = if before {
                [unknown.as_slice(), sni].concat()
            } else {
                [sni, &unknown].concat()
            };
            assert_eq!(
                extract_sni(&replace_extensions(&extensions)),
                Some("example.com".into())
            );
            for malformed in [&[0x12][..], &[0x12, 0x34], &[0x12, 0x34, 0], &[0x12, 0x34, 0, 2, 0xaa]] {
                let extensions = if before {
                    [malformed, sni].concat()
                } else {
                    [sni, malformed].concat()
                };
                assert_eq!(extract_sni(&replace_extensions(&extensions)), None);
            }
        }
    }

    #[test]
    fn test_sni_list_validates_entries_after_hostname() {
        let original = build_client_hello("example.com");
        for tail in [&[1][..], &[1, 0], &[1, 0, 2, 0xaa]] {
            let mut sni = original[SNI_EXT..].to_vec();
            sni.extend_from_slice(tail);
            let len = sni.len();
            set_u16(&mut sni, 2, len - 4);
            set_u16(&mut sni, 4, len - 6);
            assert_eq!(extract_sni(&replace_extensions(&sni)), None);
        }
    }

    #[test]
    fn test_structural_checks_preserve_name_and_duplicate_selection() {
        let original = build_client_hello("First.EXAMPLE");
        let second = build_client_hello("second.example");
        let first_sni = &original[SNI_EXT..];
        let second_sni = &second[SNI_EXT..];
        let tls13_versions = [0, 43, 0, 3, 2, 3, 4];
        let extensions = [first_sni, &tls13_versions, second_sni].concat();
        assert_eq!(
            extract_sni(&replace_extensions(&extensions)),
            Some("first.example".into())
        );

        // Unknown names remain skippable, but their lengths must fit the list.
        let mut sni = first_sni[..6].to_vec();
        sni.extend_from_slice(&[1, 0, 2, 0xaa, 0xbb]);
        sni.extend_from_slice(&first_sni[6..]);
        sni.extend_from_slice(&second_sni[6..]);
        let len = sni.len();
        set_u16(&mut sni, 2, len - 4);
        set_u16(&mut sni, 4, len - 6);
        assert_eq!(extract_sni(&replace_extensions(&sni)), Some("first.example".into()));
        set_u16(&mut sni, 7, u16::MAX as usize);
        assert_eq!(extract_sni(&replace_extensions(&sni)), None);

        // Route input validation rejects an unusable first name; it must not
        // select a later SNI extension to repair that name.
        for name in ["", "*", "a..example", "a.example.", "a.example:443", "é.example"] {
            assert_eq!(extract_sni(&build_client_hello(name)), None, "{name}");
        }
        let unknown_only = [0, 0, 0, 6, 0, 4, 1, 0, 1, 0xaa];
        assert_eq!(
            extract_sni(&replace_extensions(&[&unknown_only, second_sni].concat())),
            None
        );
        let mut invalid_utf8 = first_sni.to_vec();
        invalid_utf8[9] = 0xff;
        assert_eq!(
            extract_sni(&replace_extensions(&[invalid_utf8.as_slice(), second_sni].concat())),
            None
        );
    }

    #[test]
    fn test_handshake_and_record_payload_isolation() {
        let original = build_client_hello("example.com");
        let mut extra_handshake = original.clone();
        extra_handshake.extend_from_slice(&[2, 0, 0, 1, 0xaa]);
        let len = extra_handshake.len();
        set_u16(&mut extra_handshake, 3, len - 5);
        assert_eq!(extract_sni(&extra_handshake), Some("example.com".into()));

        // Bytes after the first declared handshake cannot supply its extensions.
        set_handshake_len(&mut extra_handshake, EXTENSIONS_LEN - 9);
        assert_eq!(extract_sni(&extra_handshake), None);

        let mut early_data = original.clone();
        early_data.extend_from_slice(&[0x17, 3, 3, 0, 2, 0xaa, 0xbb]);
        assert_eq!(extract_sni(&early_data), Some("example.com".into()));
        assert_eq!(
            client_hello_record_complete(&early_data),
            RecordCompleteness::Complete {
                record_end: original.len()
            }
        );
        set_u16(&mut early_data, 3, len + 100);
        assert_eq!(client_hello_record_complete(&early_data), RecordCompleteness::NeedMore);
        assert_eq!(extract_sni(&early_data), None);
    }

    #[test]
    fn test_extract_sni_subdomain() {
        let buf = build_client_hello("api.staging.example.com");
        assert_eq!(extract_sni(&buf), Some("api.staging.example.com".to_string()));
    }

    #[test]
    fn test_extract_sni_uppercase_normalized() {
        let buf = build_client_hello("Example.COM");
        assert_eq!(extract_sni(&buf), Some("example.com".to_string()));
    }

    #[test]
    fn test_extract_sni_empty_buffer() {
        assert_eq!(extract_sni(&[]), None);
    }

    #[test]
    fn test_extract_sni_not_handshake() {
        // ContentType = 0x17 (Application Data)
        let buf = vec![0x17, 0x03, 0x03, 0x00, 0x05, 0x01, 0x00, 0x00, 0x01, 0x00];
        assert_eq!(extract_sni(&buf), None);
    }

    #[test]
    fn test_extract_sni_not_client_hello() {
        // Handshake type = 0x02 (ServerHello)
        let mut buf = build_client_hello("example.com");
        buf[5] = 0x02;
        assert_eq!(extract_sni(&buf), None);
    }

    #[test]
    fn test_extract_sni_truncated() {
        let buf = build_client_hello("example.com");
        // Truncate at various points
        assert_eq!(extract_sni(&buf[..3]), None);
        assert_eq!(extract_sni(&buf[..10]), None);
        // Truncated mid-record (full header but short fragment) must also
        // refuse parsing rather than clamping to whatever bytes are present.
        assert_eq!(extract_sni(&buf[..buf.len() - 1]), None);
    }

    #[test]
    fn test_record_complete_states() {
        // Empty buffer: caller must keep reading.
        assert_eq!(client_hello_record_complete(&[]), RecordCompleteness::NeedMore);

        // First byte not 0x16: cannot become a TLS Handshake record.
        assert_eq!(
            client_hello_record_complete(&[0x17, 0x03, 0x03, 0x00, 0x05]),
            RecordCompleteness::NotTlsHandshake
        );

        // First byte 0x16 but header not yet complete (< 5 bytes).
        assert_eq!(
            client_hello_record_complete(&[0x16, 0x03, 0x01]),
            RecordCompleteness::NeedMore
        );

        // Header announces record_len=3000 but only 1500 bytes arrived.
        let mut short = vec![0x16, 0x03, 0x01, 0x0b, 0xb8]; // 0x0bb8 = 3000
        short.extend(std::iter::repeat_n(0u8, 1500 - 5));
        assert_eq!(client_hello_record_complete(&short), RecordCompleteness::NeedMore);

        // Buffer is exactly the record, no trailing byte.
        let buf = build_client_hello("example.com");
        match client_hello_record_complete(&buf) {
            RecordCompleteness::Complete { record_end } => {
                assert_eq!(record_end, buf.len());
            }
            other => panic!("expected Complete (exact), got {other:?}"),
        }

        // Buffer contains the full record and one extra trailing byte.
        let mut with_tail = buf.clone();
        with_tail.push(0xaa);
        match client_hello_record_complete(&with_tail) {
            RecordCompleteness::Complete { record_end } => {
                assert_eq!(record_end, buf.len());
            }
            other => panic!("expected Complete (with tail), got {other:?}"),
        }
    }

    #[test]
    fn test_extract_sni_refuses_truncated_record_with_parseable_prefix() {
        // Inflate the declared record length by +500 bytes without changing the
        // buffer contents — the buffer still holds an end-to-end parseable SNI
        // extension, but the record header claims the record extends past the
        // buffer end. The parser must refuse this as a truncated record per
        // RFC 8446 §5.1 and RFC 6066 §3 and return None rather than parsing
        // from the incomplete record.
        let mut buf = build_client_hello("example.com");
        let original_len = u16::from_be_bytes([buf[3], buf[4]]);
        let inflated = original_len + 500;
        buf[3] = (inflated >> 8) as u8;
        buf[4] = (inflated & 0xff) as u8;

        // Sanity: completeness probe agrees the record header now claims
        // more bytes than have arrived, so this is a "need more bytes"
        // state — not a NotTlsHandshake or Complete state.
        assert!(matches!(
            client_hello_record_complete(&buf),
            RecordCompleteness::NeedMore
        ));
        assert_eq!(extract_sni(&buf), None);
    }

    #[test]
    fn test_extract_sni_refuses_truncated_3000_byte_record() {
        // Header declares 3000 bytes of fragment but only 1500 arrived
        // and the payload is all-zero (i.e. not even a ClientHello body).
        // The parser must return None — covers the "truncated header" path
        // even when no exploit payload is present.
        let mut short = vec![0x16, 0x03, 0x01, 0x0b, 0xb8];
        short.extend(std::iter::repeat_n(0u8, 1500 - 5));
        assert_eq!(extract_sni(&short), None);
    }

    #[test]
    fn test_extract_sni_no_extensions() {
        // Build a ClientHello with no extensions section
        let mut hello_body = Vec::new();
        hello_body.extend_from_slice(&[0x03, 0x03]); // version
        hello_body.extend_from_slice(&[0u8; 32]); // random
        hello_body.push(0); // session_id length = 0
        hello_body.extend_from_slice(&[0x00, 0x02, 0x00, 0x2f]); // cipher suites
        hello_body.extend_from_slice(&[0x01, 0x00]); // compression

        let mut handshake = vec![0x01];
        let len = hello_body.len();
        handshake.push((len >> 16) as u8);
        handshake.push((len >> 8) as u8);
        handshake.push(len as u8);
        handshake.extend_from_slice(&hello_body);

        let mut record = vec![0x16, 0x03, 0x01];
        record.extend_from_slice(&(handshake.len() as u16).to_be_bytes());
        record.extend_from_slice(&handshake);

        assert_eq!(extract_sni(&record), None);
    }
}
