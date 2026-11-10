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
///
/// Only the leading/trailing rules depend on position. Every structural
/// character is escaped wherever it occurs, so the result may also be spliced
/// into the middle of a template value, provided the surrounding template text
/// is itself valid RFC 4514 (for example it does not end in a lone `\`).
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
            // Only ASCII bytes are escaped, so a non-ASCII character is
            // always copied whole.
            Ok(byte) if byte.is_ascii() && needs_escape(byte, index, len) => {
                out.push('\\');
                out.push(hex_digit(byte >> 4));
                out.push(hex_digit(byte & 0x0f));
            }
            _ => out.push(ch),
        }
    }
    Cow::Owned(out)
}

/// A normalized attribute value. `Text` is a string value after RFC 4518
/// mapping, lowercasing and space handling; `Opaque` is a well-formed `#hex`
/// BER element that is not a text string, compared bytewise. The two kinds
/// never compare equal.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Value {
    Text(Vec<u8>),
    Opaque(Vec<u8>),
}

/// One attribute-value assertion of a normalized DN: canonical type name and
/// normalized value.
type Ava = (Vec<u8>, Value);

/// A parsed DN: normalized RDNs, and whether the text used only the plain
/// string form (descriptor types, no `#hex` values).
struct Parsed {
    rdns: Vec<Vec<Ava>>,
    string_form: bool,
}

/// A DN normalized for comparison (see [`equivalent`]). Two DNs compare equal
/// exactly when [`equivalent`] holds for their source strings. `Debug`
/// redacts the content: a found user DN must never be logged.
#[derive(Clone, PartialEq, Eq)]
pub struct NormalizedDn(Vec<Vec<Ava>>);

impl std::fmt::Debug for NormalizedDn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("NormalizedDn([redacted])")
    }
}

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

/// Canonical names for common naming attributes, keyed by lowercase
/// descriptor or numeric OID, so `commonName=x` and `2.5.4.3=x` compare equal
/// to `cn=x`. Other types compare by their lowercase spelling. Sources: RFC
/// 4519, RFC 4524, RFC 4512 (`aliasedObjectName`) and RFC 2985 (`email`).
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
    (b"rfc822mailbox", b"mail"),
    (b"0.9.2342.19200300.100.1.3", b"mail"),
    (b"emailaddress", b"email"),
    (b"pkcs9email", b"email"),
    (b"1.2.840.113549.1.9.1", b"email"),
    (b"mobiletelephonenumber", b"mobile"),
    (b"0.9.2342.19200300.100.1.41", b"mobile"),
    (b"fax", b"facsimiletelephonenumber"),
    (b"2.5.4.23", b"facsimiletelephonenumber"),
    (b"friendlycountryname", b"co"),
    (b"0.9.2342.19200300.100.1.43", b"co"),
    (b"aliasedentryname", b"aliasedobjectname"),
    (b"2.5.4.1", b"aliasedobjectname"),
    (b"2.5.4.5", b"serialnumber"),
    (b"2.5.4.41", b"name"),
    (b"2.5.4.43", b"initials"),
    (b"2.5.4.44", b"generationqualifier"),
    (b"2.5.4.46", b"dnqualifier"),
    (b"2.5.4.65", b"pseudonym"),
    (b"2.5.4.20", b"telephonenumber"),
    (b"0.9.2342.19200300.100.1.44", b"uniqueidentifier"),
    (b"2.5.4.15", b"businesscategory"),
    (b"2.5.4.17", b"postalcode"),
];

fn canonical_type(attribute_type: &[u8]) -> Vec<u8> {
    let lower = attribute_type.to_ascii_lowercase();
    TYPE_ALIASES
        .iter()
        .find(|(alias, _)| *alias == lower.as_slice())
        .map_or(lower, |(_, short)| short.to_vec())
}

/// Value of a `#hex` attribute value: exactly one BER element with a
/// single-octet identifier and a short-form length or a long-form length of one
/// or two octets, else `None` (the DN is then invalid). String types whose
/// content is valid text become [`Value::Text`]; any other element is
/// [`Value::Opaque`] over its raw bytes.
fn decode_hex_value(raw: &[u8]) -> Option<Value> {
    let (&tag, rest) = raw.split_first()?;
    // High-tag-number identifiers (multi-byte tags) are not supported.
    if tag & 0x1f == 0x1f {
        return None;
    }
    let (&first, rest) = rest.split_first()?;
    let (len, content) = match (first, rest) {
        (0x00..=0x7f, content) => (usize::from(first), content),
        (0x81, [len, content @ ..]) => (usize::from(*len), content),
        (0x82, [high, low, content @ ..]) => ((usize::from(*high) << 8) | usize::from(*low), content),
        _ => return None,
    };
    if content.len() != len {
        return None;
    }
    let folded = match tag {
        // OCTET STRING, UTF8String, PrintableString, TeletexString, IA5String, VisibleString.
        0x04 | 0x0c | 0x13 | 0x14 | 0x16 | 0x1a => std::str::from_utf8(content).ok().map(fold_value),
        // BMPString: UTF-16BE code units.
        0x1e if len % 2 == 0 => {
            let units = content.as_chunks::<2>().0.iter().map(|pair| u16::from_be_bytes(*pair));
            char::decode_utf16(units)
                .collect::<Result<String, _>>()
                .ok()
                .as_deref()
                .map(fold_value)
        }
        // UniversalString: UTF-32BE code points.
        0x1c if len % 4 == 0 => content
            .as_chunks::<4>()
            .0
            .iter()
            .map(|quad| char::from_u32(u32::from_be_bytes(*quad)))
            .collect::<Option<String>>()
            .as_deref()
            .map(fold_value),
        _ => None,
    };
    Some(folded.map_or_else(|| Value::Opaque(raw.to_vec()), Value::Text))
}

/// RFC 4518 section 2.2 map of one character: `None` maps it to nothing,
/// otherwise the (possibly space-mapped) character. Code points are written
/// numerically: several are invisible or bidirectional controls. Entries are
/// control, format, separator, combining-joiner and variation-selector code
/// points, never script letters; the table is a superset of the Unicode 3.2
/// set RFC 4518 references (U+0600..U+0603 became format characters later).
fn rfc4518_map(ch: char) -> Option<char> {
    match u32::from(ch) {
        0x0009..=0x000D | 0x0085 | 0x00A0 | 0x1680 | 0x2000..=0x200A | 0x2028 | 0x2029 | 0x202F | 0x205F | 0x3000 => {
            Some(' ')
        }
        0x0000..=0x0008
        | 0x000E..=0x001F
        | 0x007F..=0x0084
        | 0x0086..=0x009F
        | 0x00AD
        | 0x034F
        | 0x0600..=0x0603
        | 0x06DD
        | 0x070F
        | 0x1806
        | 0x180B..=0x180E
        | 0x200B..=0x200F
        | 0x202A..=0x202E
        | 0x2060..=0x2063
        | 0x206A..=0x206F
        | 0xFE00..=0xFE0F
        | 0xFEFF
        | 0xFFF9..=0xFFFC
        | 0x1D173..=0x1D17A
        | 0xE0001
        | 0xE0020..=0xE007F => None,
        _ => Some(ch),
    }
}

/// Normalize a text value: RFC 4518 mapping, context-free per-character
/// lowercasing with final sigma unified, then insignificant-space handling
/// (leading and trailing spaces dropped, internal runs collapsed to one).
fn fold_value(text: &str) -> Vec<u8> {
    const FINAL_SIGMA: char = '\u{03C2}';
    const SIGMA: char = '\u{03C3}';
    let mut mapped = String::with_capacity(text.len());
    for ch in text.chars().filter_map(rfc4518_map) {
        mapped.extend(
            ch.to_lowercase()
                .map(|lower| if lower == FINAL_SIGMA { SIGMA } else { lower }),
        );
    }
    let mut out = Vec::with_capacity(mapped.len());
    for word in mapped.split(' ').filter(|word| !word.is_empty()) {
        if !out.is_empty() {
            out.push(b' ');
        }
        out.extend_from_slice(word.as_bytes());
    }
    out
}

/// Parse a DN string (RFC 4514) into normalized RDNs, iteratively.
///
/// Types map to a canonical name ([`TYPE_ALIASES`], else lowercase); string
/// values have `\XX` and `\c` escapes decoded and must be UTF-8; `#hex`
/// values go through [`decode_hex_value`]; text is folded by [`fold_value`];
/// the AVAs of a multi-valued RDN are sorted. Returns `None` for an empty DN,
/// an invalid attribute type, a malformed escape, a non-UTF-8 string value, a
/// `#hex` value that is not one well-formed element, an unescaped `" < >` or
/// NUL, or text after a `#hex` value that is not a separator. `strict` (the
/// admission entry points) also rejects the RFC 2253 `;` separator.
fn parse(dn: &str, strict: bool) -> Option<Parsed> {
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
        let normalized = if bytes.get(pos) == Some(&b'#') {
            string_form = false;
            pos += 1;
            while pos < bytes.len() && !matches!(bytes[pos], b',' | b'+' | b';' | b' ') {
                let high = hex_value(bytes[pos])?;
                let low = hex_value(*bytes.get(pos + 1)?)?;
                value.push((high << 4) | low);
                pos += 2;
            }
            let decoded = decode_hex_value(&value)?;
            while bytes.get(pos) == Some(&b' ') {
                pos += 1;
            }
            decoded
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
            Value::Text(fold_value(&String::from_utf8(value).ok()?))
        };
        rdn.push((canonical_type(attribute_type), normalized));
        match bytes.get(pos) {
            None => {
                rdn.sort();
                rdns.push(rdn);
                return Some(Parsed { rdns, string_form });
            }
            Some(b'+') => pos += 1,
            Some(b';') if strict => return None,
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
///
/// Lenient for directory-returned DNs: the RFC 2253 `;` separator is accepted.
pub fn normalize(dn: &str) -> Option<NormalizedDn> {
    parse(dn, false).map(|parsed| NormalizedDn(parsed.rdns))
}

/// Whether `dn` parses as a non-empty RFC 4514 distinguished name (admission
/// check: the RFC 2253 `;` separator is rejected).
pub fn is_valid(dn: &str) -> bool {
    parse(dn, true).is_some()
}

/// Whether `dn` is valid (as [`is_valid`]) and written in the plain string
/// form: descriptor attribute types (`cn`, not `2.5.4.3`) and no `#hex` values.
pub fn is_string_form(dn: &str) -> bool {
    parse(dn, true).is_some_and(|parsed| parsed.string_form)
}

/// Whether two DNs compare equal after normalization: canonical names for
/// common attribute types and their OIDs, decoded escapes and `#hex` strings,
/// RFC 4518 mapping, per-character lowercase values, insignificant-space
/// handling and AVA order.
///
/// This approximates the directory's `distinguishedNameMatch`; it does not
/// apply full Unicode case folding or NFKC, map control characters newer than
/// the fixed RFC 4518 table, or know schema aliases beyond the module's alias
/// table. Unparsable input is never equivalent.
pub fn equivalent(a: &str, b: &str) -> bool {
    match (normalize(a), normalize(b)) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

#[cfg(test)]
#[path = "../tests/unit/dn_test.rs"]
mod tests;
