//! LDAPMessage encoders and decoders for bind, unbind and StartTLS.
//!
//! Decoders take the content of one frame (the bytes after the outer
//! `SEQUENCE` header) and run straight-line code of fixed depth: LDAPMessage,
//! protocolOp, LDAPResult. Optional trailing elements are skipped by length.

use crate::codec::{
    element_len, u32_content_len, write_header, write_octets, write_u32, Reader, BIND_REQUEST, BIND_RESPONSE,
    CONTEXT_0_PRIMITIVE, ENUMERATED, EXTENDED_REQUEST, EXTENDED_RESPONSE, INTEGER, OCTET_STRING, SEQUENCE,
    UNBIND_REQUEST,
};
use crate::Error;

/// LDAP protocol version sent in every BindRequest.
const LDAP_VERSION: u32 = 3;
/// StartTLS extended operation name (RFC 4511 section 4.14.1).
const START_TLS_OID: &[u8] = b"1.3.6.1.4.1.1466.20037";

/// The only LDAPResult component Edgion consumes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LdapResult {
    pub result_code: u32,
}

/// Write the LDAPMessage envelope around a protocolOp of `op_len` content bytes.
fn message(msg_id: u32, op_tag: u8, op_len: usize, write_op: impl FnOnce(&mut Vec<u8>)) -> Vec<u8> {
    let content_len = element_len(u32_content_len(msg_id)) + element_len(op_len);
    let mut out = Vec::with_capacity(element_len(content_len));
    write_header(&mut out, SEQUENCE, content_len);
    write_u32(&mut out, INTEGER, msg_id);
    write_header(&mut out, op_tag, op_len);
    write_op(&mut out);
    out
}

/// `BindRequest ::= [APPLICATION 0] { version 3, name, simple [0] password }`.
pub(crate) fn encode_bind_request(msg_id: u32, bind_dn: &str, password: &[u8]) -> Vec<u8> {
    let op_len = element_len(u32_content_len(LDAP_VERSION)) + element_len(bind_dn.len()) + element_len(password.len());
    message(msg_id, BIND_REQUEST, op_len, |out| {
        write_u32(out, INTEGER, LDAP_VERSION);
        write_octets(out, OCTET_STRING, bind_dn.as_bytes());
        write_octets(out, CONTEXT_0_PRIMITIVE, password);
    })
}

/// `UnbindRequest ::= [APPLICATION 2] NULL`.
pub(crate) fn encode_unbind_request(msg_id: u32) -> Vec<u8> {
    message(msg_id, UNBIND_REQUEST, 0, |_| {})
}

/// `ExtendedRequest ::= [APPLICATION 23] { requestName [0] StartTLS OID }`.
pub(crate) fn encode_start_tls_request(msg_id: u32) -> Vec<u8> {
    message(msg_id, EXTENDED_REQUEST, element_len(START_TLS_OID.len()), |out| {
        write_octets(out, CONTEXT_0_PRIMITIVE, START_TLS_OID);
    })
}

/// Decode a BindResponse frame content answering message `expected_id`.
pub fn decode_bind_response(content: &[u8], expected_id: u32) -> Result<LdapResult, Error> {
    decode_result(content, expected_id, BIND_RESPONSE)
}

/// Decode an ExtendedResponse frame content answering message `expected_id`.
pub fn decode_extended_response(content: &[u8], expected_id: u32) -> Result<LdapResult, Error> {
    decode_result(content, expected_id, EXTENDED_RESPONSE)
}

fn decode_result(content: &[u8], expected_id: u32, op_tag: u8) -> Result<LdapResult, Error> {
    let mut message = Reader::new(content);
    let actual = message.read_u32(INTEGER)?;
    if actual != expected_id {
        return Err(Error::UnexpectedMessageId {
            expected: expected_id,
            actual,
        });
    }
    let mut op = message.expect(op_tag)?;
    let result_code = op.read_u32(ENUMERATED)?;
    // matchedDN and diagnosticMessage: validated, never retained.
    op.read_utf8_octets()?;
    op.read_utf8_octets()?;
    // referral [3], serverSaslCreds [7], responseName [10], responseValue [11], unknown.
    while !op.is_empty() {
        op.skip_element()?;
    }
    // controls [0] and any other trailing element, such as a [10] element
    // shaped like Active Directory's misplaced Notice-of-Disconnection OID. A
    // real Notice of Disconnection carries message ID 0 and is rejected above.
    while !message.is_empty() {
        message.skip_element()?;
    }
    Ok(LdapResult { result_code })
}

#[cfg(test)]
#[path = "../tests/unit/proto_test.rs"]
mod tests;
