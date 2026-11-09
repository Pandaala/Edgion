//! RFC 4514 distinguished names: value escaping, validation and the
//! normalized comparison used by the search-mode service-account guard.

use std::borrow::Cow;

use crate::filter::{hex_value, is_keystring, is_numeric_oid};

fn always_escape(byte: u8) -> bool {
    matches!(byte, b'"' | b'+' | b',' | b';' | b'<' | b'=' | b'>' | b'\\' | 0)
}

fn needs_escape(byte: u8, index: usize, len: usize) -> bool {
    always_escape(byte) || (index == 0 && matches!(byte, b' ' | b'#')) || (index + 1 == len && byte == b' ')
}

fn hex_digit(nibble: u8) -> char {
    char::from(if nibble < 10 { b'0' + nibble } else { b'a' + nibble - 10 })
}

/// Escape `value` for use as an RDN attribute value.
///
/// Byte-identical to `ldap3::dn_escape` 0.12: the special characters
/// `" + , ; < = > \` and NUL anywhere, a leading space or `#`, and a trailing
/// space become lowercase `\HH`. Only ASCII bytes are ever escaped, so the
/// output stays valid UTF-8. Returns the input unchanged when nothing needs
/// escaping.
pub fn escape(value: &str) -> Cow<'_, str> {
    let len = value.len();
    if !value
        .bytes()
        .enumerate()
        .any(|(index, byte)| needs_escape(byte, index, len))
    {
        return Cow::Borrowed(value);
    }
    let mut out = String::with_capacity(len + 12);
    for (index, ch) in value.char_indices() {
        match u8::try_from(ch) {
            // `needs_escape` only matches ASCII bytes, so a multi-byte
            // character is always copied whole.
            Ok(byte) if needs_escape(byte, index, len) => {
                out.push('\\');
                out.push(hex_digit(byte >> 4));
                out.push(hex_digit(byte & 0x0f));
            }
            _ => out.push(ch),
        }
    }
    Cow::Owned(out)
}

/// One attribute-value assertion of a normalized DN: canonical type name and
/// normalized value bytes.
type Ava = (Vec<u8>, Vec<u8>);

/// A parsed DN: normalized RDNs, and whether the text used only the plain
/// string form (descriptor types, no `#hex` values).
struct Parsed {
    rdns: Vec<Vec<Ava>>,
    string_form: bool,
}

/// A DN normalized for comparison (see [`equivalent`]). Two DNs compare equal
/// exactly when [`equivalent`] holds for their source strings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedDn(Vec<Vec<Ava>>);

/// Characters that may follow a backslash as a literal (RFC 4514 section 3).
fn is_escapable(byte: u8) -> bool {
    matches!(
        byte,
        b' ' | b'"' | b'#' | b'+' | b',' | b';' | b'<' | b'=' | b'>' | b'\\'
    )
}

/// Characters that RFC 4514 requires to be escaped inside a string value
/// (`+ , ;` end the value instead).
fn must_be_escaped(byte: u8) -> bool {
    matches!(byte, b'"' | b'<' | b'>' | 0)
}

/// Canonical short names for the common naming attributes, keyed by lowercase
/// descriptor or numeric OID, so `commonName=x` and `2.5.4.3=x` compare equal
/// to `cn=x`. Other types compare by their lowercase spelling.
const TYPE_ALIASES: &[(&[u8], &[u8])] = &[
    (b"commonname", b"cn"),
    (b"2.5.4.3", b"cn"),
    (b"surname", b"sn"),
    (b"2.5.4.4", b"sn"),
    (b"countryname", b"c"),
    (b"2.5.4.6", b"c"),
    (b"localityname", b"l"),
    (b"2.5.4.7", b"l"),
    (b"stateorprovincename", b"st"),
    (b"2.5.4.8", b"st"),
    (b"streetaddress", b"street"),
    (b"2.5.4.9", b"street"),
    (b"organizationname", b"o"),
    (b"2.5.4.10", b"o"),
    (b"organizationalunitname", b"ou"),
    (b"2.5.4.11", b"ou"),
    (b"2.5.4.12", b"title"),
    (b"gn", b"givenname"),
    (b"2.5.4.42", b"givenname"),
    (b"userid", b"uid"),
    (b"0.9.2342.19200300.100.1.1", b"uid"),
    (b"domaincomponent", b"dc"),
    (b"0.9.2342.19200300.100.1.25", b"dc"),
];

fn canonical_type(attribute_type: &[u8]) -> Vec<u8> {
    let lower = attribute_type.to_ascii_lowercase();
    TYPE_ALIASES
        .iter()
        .find(|(alias, _)| *alias == lower.as_slice())
        .map_or(lower, |(_, short)| short.to_vec())
}

/// Content octets of a BER-encoded string value (`#hex` form), when it is a
/// single primitive string element; otherwise the raw bytes.
fn ber_string_content(mut raw: Vec<u8>) -> Vec<u8> {
    let header = match raw.as_slice() {
        [0x04 | 0x0c | 0x13 | 0x14 | 0x16 | 0x1a, len, rest @ ..] if *len < 0x80 && rest.len() == usize::from(*len) => {
            2
        }
        [0x04 | 0x0c | 0x13 | 0x14 | 0x16 | 0x1a, 0x81, len, rest @ ..] if rest.len() == usize::from(*len) => 3,
        _ => 0,
    };
    raw.drain(..header);
    raw
}

/// Case-fold a decoded value and apply RFC 4518 insignificant-space handling:
/// leading and trailing spaces dropped, internal runs collapsed to one.
fn fold_value(value: Vec<u8>) -> Vec<u8> {
    let folded = match String::from_utf8(value) {
        Ok(text) => text.to_lowercase().into_bytes(),
        Err(error) => {
            let mut bytes = error.into_bytes();
            bytes.make_ascii_lowercase();
            bytes
        }
    };
    let mut out = Vec::with_capacity(folded.len());
    for word in folded.split(|&b| b == b' ').filter(|word| !word.is_empty()) {
        if !out.is_empty() {
            out.push(b' ');
        }
        out.extend_from_slice(word);
    }
    out
}

/// Parse a DN string (RFC 4514) into normalized RDNs, iteratively.
///
/// Types map to a canonical name ([`TYPE_ALIASES`], else lowercase); values
/// have `\XX` and `\c` escapes decoded, `#hex` BER strings unwrapped, Unicode
/// case folded with `to_lowercase` and spaces normalized as in RFC 4518; the
/// AVAs of a multi-valued RDN are sorted. Returns `None` for an empty DN, an
/// invalid attribute type, a malformed escape or hex value, an unescaped
/// `" < >` or NUL, or text after a `#hex` value that is not a separator.
fn parse(dn: &str) -> Option<Parsed> {
    let bytes = dn.as_bytes();
    let mut rdns: Vec<Vec<Ava>> = Vec::new();
    let mut rdn: Vec<Ava> = Vec::new();
    let mut string_form = true;
    let mut pos = 0usize;
    loop {
        while bytes.get(pos) == Some(&b' ') {
            pos += 1;
        }
        let type_start = pos;
        while pos < bytes.len() && bytes[pos] != b'=' && bytes[pos] != b' ' {
            pos += 1;
        }
        let attribute_type = &bytes[type_start..pos];
        if is_numeric_oid(attribute_type) {
            string_form = false;
        } else if !is_keystring(attribute_type) {
            return None;
        }
        while bytes.get(pos) == Some(&b' ') {
            pos += 1;
        }
        if bytes.get(pos) != Some(&b'=') {
            return None;
        }
        pos += 1;
        while bytes.get(pos) == Some(&b' ') {
            pos += 1;
        }
        let mut value: Vec<u8> = Vec::new();
        if bytes.get(pos) == Some(&b'#') {
            string_form = false;
            pos += 1;
            while pos < bytes.len() && !matches!(bytes[pos], b',' | b'+' | b';' | b' ') {
                let high = hex_value(bytes[pos])?;
                let low = hex_value(*bytes.get(pos + 1)?)?;
                value.push((high << 4) | low);
                pos += 2;
            }
            if value.is_empty() {
                return None;
            }
            value = ber_string_content(value);
            while bytes.get(pos) == Some(&b' ') {
                pos += 1;
            }
        } else {
            while pos < bytes.len() && !matches!(bytes[pos], b',' | b'+' | b';') {
                let byte = bytes[pos];
                if byte == b'\\' {
                    let next = *bytes.get(pos + 1)?;
                    if let (Some(high), Some(low)) = (hex_value(next), bytes.get(pos + 2).and_then(|b| hex_value(*b))) {
                        value.push((high << 4) | low);
                        pos += 3;
                    } else if is_escapable(next) {
                        value.push(next);
                        pos += 2;
                    } else {
                        return None;
                    }
                } else if must_be_escaped(byte) {
                    return None;
                } else {
                    value.push(byte);
                    pos += 1;
                }
            }
        }
        rdn.push((canonical_type(attribute_type), fold_value(value)));
        match bytes.get(pos) {
            None => {
                rdn.sort();
                rdns.push(rdn);
                return Some(Parsed { rdns, string_form });
            }
            Some(b'+') => pos += 1,
            Some(b',' | b';') => {
                rdn.sort();
                rdns.push(std::mem::take(&mut rdn));
                pos += 1;
            }
            Some(_) => return None,
        }
    }
}

/// Normalize `dn` for comparison; `None` when it does not parse.
pub fn normalize(dn: &str) -> Option<NormalizedDn> {
    parse(dn).map(|parsed| NormalizedDn(parsed.rdns))
}

/// Whether `dn` parses as a non-empty RFC 4514 distinguished name.
pub fn is_valid(dn: &str) -> bool {
    parse(dn).is_some()
}

/// Whether `dn` is valid and written in the plain string form: descriptor
/// attribute types (`cn`, not `2.5.4.3`) and no `#hex` values.
pub fn is_string_form(dn: &str) -> bool {
    parse(dn).is_some_and(|parsed| parsed.string_form)
}

/// Whether two DNs compare equal after normalization: canonical names for
/// common attribute types and their OIDs, decoded escapes and `#hex` strings,
/// Unicode lowercase values, RFC 4518 space handling and AVA order.
///
/// This approximates the directory's `distinguishedNameMatch`; it does not
/// apply full Unicode normalization or know schema aliases beyond the
/// module's alias table. Unparsable input is never equivalent.
pub fn equivalent(a: &str, b: &str) -> bool {
    match (normalize(a), normalize(b)) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

#[cfg(test)]
#[path = "../tests/unit/dn_test.rs"]
mod tests;
