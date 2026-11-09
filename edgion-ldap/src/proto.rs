//! LDAPMessage encoders and decoders for bind, unbind, StartTLS and search.
//!
//! Decoders take the content of one frame (the bytes after the outer
//! `SEQUENCE` header) and run straight-line code of fixed depth: LDAPMessage,
//! protocolOp, LDAPResult. Optional trailing elements are skipped by length.

#[cfg(feature = "client")]
use crate::client::Scope;
#[cfg(feature = "client")]
use crate::codec::SEARCH_REQUEST;
#[cfg(feature = "client")]
use crate::codec::{
    element_len, u32_content_len, write_header, write_octets, write_u32, BIND_REQUEST, CONTEXT_0_PRIMITIVE,
    EXTENDED_REQUEST, OCTET_STRING, SEQUENCE, UNBIND_REQUEST,
};
use crate::codec::{
    Reader, BIND_RESPONSE, ENUMERATED, EXTENDED_RESPONSE, INTEGER, OCTET_STRING as OCTETS, SEARCH_RESULT_DONE,
    SEARCH_RESULT_ENTRY, SEARCH_RESULT_REFERENCE, SEQUENCE as SEQ, SET,
};
#[cfg(feature = "client")]
use crate::filter::SearchFilter;
use crate::Error;

#[cfg(feature = "client")]
/// LDAP protocol version sent in every BindRequest.
const LDAP_VERSION: u32 = 3;
#[cfg(feature = "client")]
/// StartTLS extended operation name (RFC 4511 section 4.14.1).
const START_TLS_OID: &[u8] = b"1.3.6.1.4.1.1466.20037";

/// The only LDAPResult component Edgion consumes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LdapResult {
    pub result_code: u32,
}

#[cfg(feature = "client")]
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

#[cfg(feature = "client")]
/// `BindRequest ::= [APPLICATION 0] { version 3, name, simple [0] password }`.
///
/// The buffer holds the password, so it is returned wrapped to be zeroized on
/// drop. `message` allocates the exact length, so no reallocation leaves a copy.
pub(crate) fn encode_bind_request(msg_id: u32, bind_dn: &str, password: &[u8]) -> zeroize::Zeroizing<Vec<u8>> {
    let op_len = element_len(u32_content_len(LDAP_VERSION)) + element_len(bind_dn.len()) + element_len(password.len());
    zeroize::Zeroizing::new(message(msg_id, BIND_REQUEST, op_len, |out| {
        write_u32(out, INTEGER, LDAP_VERSION);
        write_octets(out, OCTET_STRING, bind_dn.as_bytes());
        write_octets(out, CONTEXT_0_PRIMITIVE, password);
    }))
}

#[cfg(feature = "client")]
/// `UnbindRequest ::= [APPLICATION 2] NULL`.
pub(crate) fn encode_unbind_request(msg_id: u32) -> Vec<u8> {
    message(msg_id, UNBIND_REQUEST, 0, |_| {})
}

#[cfg(feature = "client")]
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

/// Read the messageID and check that it answers `expected_id`.
fn read_message_id(message: &mut Reader<'_>, expected_id: u32) -> Result<(), Error> {
    let actual = message.read_u32(INTEGER)?;
    if actual != expected_id {
        return Err(Error::UnexpectedMessageId {
            expected: expected_id,
            actual,
        });
    }
    Ok(())
}

/// Read the LDAPResult components from a protocolOp's content.
fn read_ldap_result(mut op: Reader<'_>) -> Result<LdapResult, Error> {
    let result_code = op.read_u32(ENUMERATED)?;
    // matchedDN and diagnosticMessage: validated, never retained.
    op.read_utf8_octets()?;
    op.read_utf8_octets()?;
    // referral [3], serverSaslCreds [7], responseName [10], responseValue [11], unknown.
    while !op.is_empty() {
        op.skip_element()?;
    }
    Ok(LdapResult { result_code })
}

/// Skip trailing message-level elements: controls [0] and any other trailing
/// element, such as a [10] element shaped like Active Directory's misplaced
/// Notice-of-Disconnection OID. A real Notice of Disconnection carries
/// message ID 0 and is rejected by [`read_message_id`].
fn skip_trailing(mut message: Reader<'_>) -> Result<(), Error> {
    while !message.is_empty() {
        message.skip_element()?;
    }
    Ok(())
}

fn decode_result(content: &[u8], expected_id: u32, op_tag: u8) -> Result<LdapResult, Error> {
    let mut message = Reader::new(content);
    read_message_id(&mut message, expected_id)?;
    let result = read_ldap_result(message.expect(op_tag)?)?;
    skip_trailing(message)?;
    Ok(result)
}

/// `SearchRequest ::= [APPLICATION 3] { baseObject, scope, derefAliases
/// neverDerefAliases, sizeLimit, timeLimit, typesOnly FALSE, filter,
/// attributes { attribute } }`: wholeSubtree with sizeLimit 2, or
/// baseObject with sizeLimit 1.
#[cfg(feature = "client")]
pub(crate) fn encode_search_request(
    msg_id: u32,
    base: &str,
    scope: Scope,
    filter: &SearchFilter,
    username: &[u8],
    attribute: &str,
    time_limit_secs: u32,
) -> Vec<u8> {
    const NEVER_DEREF_ALIASES: u32 = 0;
    const TYPES_ONLY_FALSE: [u8; 3] = [0x01, 0x01, 0x00];
    let (scope, size_limit) = match scope {
        Scope::Subtree => (2u32, 2u32),
        Scope::Base => (0, 1),
    };
    let attributes_len = element_len(attribute.len());
    let op_len = element_len(base.len())
        + element_len(u32_content_len(scope))
        + element_len(u32_content_len(NEVER_DEREF_ALIASES))
        + element_len(u32_content_len(size_limit))
        + element_len(u32_content_len(time_limit_secs))
        + TYPES_ONLY_FALSE.len()
        + filter.encoded_len(username.len())
        + element_len(attributes_len);
    message(msg_id, SEARCH_REQUEST, op_len, |out| {
        write_octets(out, OCTET_STRING, base.as_bytes());
        write_u32(out, ENUMERATED, scope);
        write_u32(out, ENUMERATED, NEVER_DEREF_ALIASES);
        write_u32(out, INTEGER, size_limit);
        write_u32(out, INTEGER, time_limit_secs);
        out.extend_from_slice(&TYPES_ONLY_FALSE);
        filter.encode(username, out);
        write_header(out, SEQUENCE, attributes_len);
        write_octets(out, OCTET_STRING, attribute.as_bytes());
    })
}

/// One decoded search response message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchFrame {
    /// A SearchResultEntry: its DN and, when the entry carries exactly one
    /// valid value for the requested attribute, that value.
    Entry { dn: String, identity: Option<String> },
    /// A SearchResultReference (continuation reference), skipped by length.
    Reference,
    /// The SearchResultDone that ends the search.
    Done(LdapResult),
}

/// Whether an identity value is usable as forwarded header text: non-empty,
/// no leading or trailing space (HTTP strips SP/HTAB around a field value; TAB
/// is a control character) and no control characters.
fn valid_identity(value: &str) -> bool {
    !value.is_empty() && value.trim_matches(' ') == value && !value.chars().any(char::is_control)
}

/// Decode one search response frame content answering message `expected_id`.
///
/// For an entry, only attributes whose type equals `attribute` (ASCII
/// case-insensitive, no options) are considered; the identity is set only
/// when they carry exactly one non-empty, control-free UTF-8 value.
pub fn decode_search_frame(content: &[u8], expected_id: u32, attribute: &str) -> Result<SearchFrame, Error> {
    let mut message = Reader::new(content);
    read_message_id(&mut message, expected_id)?;
    let (tag, op) = message.element()?;
    let frame = match tag {
        SEARCH_RESULT_ENTRY => {
            let mut entry = Reader::new(op);
            let dn = std::str::from_utf8(entry.element_with(OCTETS)?).map_err(|_| Error::InvalidUtf8)?;
            let mut attributes = entry.expect(SEQ)?;
            let mut matches = 0usize;
            let mut identity: Option<&[u8]> = None;
            while !attributes.is_empty() {
                let mut partial = attributes.expect(SEQ)?;
                let attribute_type = partial.element_with(OCTETS)?;
                let mut values = partial.expect(SET)?;
                let selected = attribute_type.eq_ignore_ascii_case(attribute.as_bytes());
                while !values.is_empty() {
                    let value = values.element_with(OCTETS)?;
                    if selected {
                        matches += 1;
                        identity = Some(value);
                    }
                }
            }
            let identity = match (matches, identity) {
                (1, Some(value)) => std::str::from_utf8(value)
                    .ok()
                    .filter(|value| valid_identity(value))
                    .map(str::to_owned),
                _ => None,
            };
            SearchFrame::Entry {
                dn: dn.to_owned(),
                identity,
            }
        }
        SEARCH_RESULT_REFERENCE => SearchFrame::Reference,
        SEARCH_RESULT_DONE => SearchFrame::Done(read_ldap_result(Reader::new(op))?),
        actual => {
            return Err(Error::UnexpectedTag {
                expected: SEARCH_RESULT_DONE,
                actual,
            })
        }
    };
    skip_trailing(message)?;
    Ok(frame)
}

#[cfg(test)]
#[path = "../tests/unit/proto_test.rs"]
mod tests;
