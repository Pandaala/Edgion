//! TLS ClientHello SNI extraction without TLS termination.
//!
//! Parses the SNI (Server Name Indication) extension from a raw TLS ClientHello
//! message. Used by TLS Passthrough mode to route connections based on SNI
//! without performing a full TLS handshake.

/// Maximum bytes to read from the stream for ClientHello parsing.
/// A typical ClientHello is 200-500 bytes; 4096 covers even large ones
/// with many cipher suites and extensions.
pub const MAX_CLIENT_HELLO_LEN: usize = 4096;

/// Extract the SNI hostname from a raw TLS ClientHello message buffer.
///
/// Returns `None` if the buffer is not a valid ClientHello, is truncated,
/// or does not contain an SNI extension.
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
        // Allow parsing with whatever we have, but cap at buf.len()
    }
    let handshake = &buf[5..buf.len().min(record_end)];

    // Handshake Header: Type(1) + Length(3)
    if handshake.len() < 4 {
        return None;
    }
    if handshake[0] != 0x01 {
        // Not a ClientHello
        return None;
    }
    let hello = &handshake[4..];

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
    let extensions_end = (pos + extensions_len).min(hello.len());

    while pos + 4 <= extensions_end {
        let ext_type = u16::from_be_bytes([hello[pos], hello[pos + 1]]);
        let ext_len = u16::from_be_bytes([hello[pos + 2], hello[pos + 3]]) as usize;
        pos += 4;

        if ext_type == 0x0000 {
            // SNI extension: ServerNameList length(2) + entries
            return parse_sni_extension(&hello[pos..extensions_end.min(pos + ext_len)]);
        }

        pos += ext_len;
    }

    None
}

/// Parse the SNI extension payload to extract the hostname.
fn parse_sni_extension(data: &[u8]) -> Option<String> {
    if data.len() < 2 {
        return None;
    }
    // ServerNameList: list_length(2) + ServerName*
    let mut pos = 2; // skip list length

    while pos + 3 <= data.len() {
        let name_type = data[pos];
        let name_len = u16::from_be_bytes([data[pos + 1], data[pos + 2]]) as usize;
        pos += 3;

        if name_type == 0x00 {
            // host_name
            if pos + name_len > data.len() {
                return None;
            }
            return std::str::from_utf8(&data[pos..pos + name_len])
                .ok()
                .map(|s| s.to_lowercase());
        }

        pos += name_len;
    }

    None
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
