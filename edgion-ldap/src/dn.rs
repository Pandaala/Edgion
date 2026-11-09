//! RFC 4514 escaping of a value placed inside a distinguished name.

use std::borrow::Cow;

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

#[cfg(test)]
#[path = "../tests/unit/dn_test.rs"]
mod tests;
