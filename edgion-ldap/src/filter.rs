//! RFC 4515 search-filter compiler with a `{username}` placeholder.
//!
//! An operator-written filter string is compiled once into a flat stream of
//! encoding operations. At request time the username is inserted as raw
//! assertion-value bytes into that already-compiled structure; it is never
//! parsed as filter text, so no username can change the filter's shape
//! (no LDAP filter injection). `{username}` is recognized only inside
//! assertion values, on the raw text before `\XX` escape decoding.
//!
//! Parsing and encoding are iterative: nesting is tracked with a fixed-size
//! stack bounded by [`MAX_DEPTH`], and the encoder computes lengths in a
//! reverse pass before writing in a forward pass. Nothing here recurses.

use crate::codec::{header_len, write_header};

/// Maximum filter text length in bytes.
pub const MAX_FILTER_LEN: usize = 4096;
/// Maximum nesting depth of filters (an item inside 15 `&`/`|`/`!` levels).
pub const MAX_DEPTH: usize = 16;
/// Maximum number of `{username}` placeholders.
pub const MAX_USERNAME_SLOTS: u8 = 8;

const PLACEHOLDER: &[u8] = b"{username}";

/// Active Directory LDAP_MATCHING_RULE_IN_CHAIN (transitive group membership).
pub const IN_CHAIN_RULE: &str = "1.2.840.113556.1.4.1941";

const TAG_AND: u8 = 0xa0;
const TAG_OR: u8 = 0xa1;
const TAG_NOT: u8 = 0xa2;
const TAG_EQUALITY: u8 = 0xa3;
const TAG_SUBSTRINGS: u8 = 0xa4;
const TAG_GREATER_OR_EQUAL: u8 = 0xa5;
const TAG_LESS_OR_EQUAL: u8 = 0xa6;
const TAG_PRESENT: u8 = 0x87;
const TAG_APPROX: u8 = 0xa8;
const TAG_EXTENSIBLE: u8 = 0xa9;
const TAG_OCTET_STRING: u8 = 0x04;
const TAG_SEQUENCE: u8 = 0x30;
const TAG_SUB_INITIAL: u8 = 0x80;
const TAG_SUB_ANY: u8 = 0x81;
const TAG_SUB_FINAL: u8 = 0x82;
const TAG_MRA_RULE: u8 = 0x81;
const TAG_MRA_TYPE: u8 = 0x82;
const TAG_MRA_VALUE: u8 = 0x83;
/// `dnAttributes [4] BOOLEAN TRUE`.
const MRA_DN_ATTRIBUTES_TRUE: [u8; 3] = [0x84, 0x01, 0xff];

/// Why a filter string was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum FilterError {
    #[error("ldap filter exceeds the maximum length")]
    TooLong,
    #[error("ldap filter nests deeper than the maximum depth")]
    TooDeep,
    #[error("ldap filter syntax is invalid")]
    Syntax,
    #[error("ldap filter attribute description or matching rule is invalid")]
    InvalidAttribute,
    #[error("ldap filter value contains an invalid escape or unescaped special character")]
    InvalidEscape,
    #[error("ldap filter must contain {{username}}")]
    NoUsername,
    #[error("ldap filter contains too many {{username}} placeholders")]
    TooManyUsernames,
    #[error("ldap filter places {{username}} outside an assertion value")]
    UsernameOutsideValue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Op {
    /// Start an element with this identifier; its content ends at the matching `Close`.
    Open(u8),
    Close,
    /// Raw content bytes.
    Bytes(Box<[u8]>),
    /// The username's raw bytes.
    Username,
}

/// A compiled search filter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchFilter {
    ops: Vec<Op>,
    slots: u8,
}

impl SearchFilter {
    /// Compile an RFC 4515 filter string containing 1 to 8 `{username}` slots.
    pub fn compile(text: &str) -> Result<Self, FilterError> {
        if text.len() > MAX_FILTER_LEN {
            return Err(FilterError::TooLong);
        }
        let mut builder = Builder::default();
        parse(text.as_bytes(), &mut builder)?;
        match builder.slots {
            0 => Err(FilterError::NoUsername),
            n if n > MAX_USERNAME_SLOTS => Err(FilterError::TooManyUsernames),
            slots => Ok(Self {
                ops: builder.ops,
                slots,
            }),
        }
    }

    /// Static filter for a group requirement on `attribute`: the entry must
    /// carry every DN of `all_of` and, when `any_of` is non-empty, at least
    /// one DN of `any_of`. Encoded as `&`(all-of items, `|`(any-of items));
    /// with `nested`, each item is an extensible match with the Active
    /// Directory in-chain rule [`IN_CHAIN_RULE`].
    ///
    /// The DNs are raw assertion values (the directory compares them with
    /// its DN syntax), so nothing in them can change the filter. The filter
    /// has no `{username}` slot (`encode` ignores the username) and is
    /// encoded once here, so each later `encode` is a single copy.
    pub fn group_requirement(
        attribute: &str,
        any_of: &[&str],
        all_of: &[&str],
        nested: bool,
    ) -> Result<Self, FilterError> {
        if !is_attribute_description(attribute) {
            return Err(FilterError::InvalidAttribute);
        }
        if any_of.len() + all_of.len() == 0 || any_of.iter().chain(all_of).any(|dn| dn.is_empty()) {
            return Err(FilterError::Syntax);
        }
        let item = |out: &mut Builder, dn: &str| {
            if nested {
                out.open(TAG_EXTENSIBLE);
                out.element(TAG_MRA_RULE, IN_CHAIN_RULE.as_bytes());
                out.element(TAG_MRA_TYPE, attribute.as_bytes());
                out.element(TAG_MRA_VALUE, dn.as_bytes());
            } else {
                out.open(TAG_EQUALITY);
                out.element(TAG_OCTET_STRING, attribute.as_bytes());
                out.element(TAG_OCTET_STRING, dn.as_bytes());
            }
            out.close();
        };
        let mut builder = Builder::default();
        builder.open(TAG_AND);
        for dn in all_of {
            item(&mut builder, dn);
        }
        if !any_of.is_empty() {
            builder.open(TAG_OR);
            for dn in any_of {
                item(&mut builder, dn);
            }
            builder.close();
        }
        builder.close();
        let built = Self {
            ops: builder.ops,
            slots: 0,
        };
        let mut encoded = Vec::with_capacity(built.encoded_len(0));
        built.encode(&[], &mut encoded);
        Ok(Self {
            ops: vec![Op::Bytes(encoded.into_boxed_slice())],
            slots: 0,
        })
    }

    /// Number of `{username}` placeholders (0 for a group requirement).
    pub fn slots(&self) -> u8 {
        self.slots
    }

    /// Append the BER encoding of the filter with `username` inserted.
    pub fn encode(&self, username: &[u8], out: &mut Vec<u8>) {
        if let [Op::Bytes(bytes)] = self.ops.as_slice() {
            out.extend_from_slice(bytes);
            return;
        }
        let mut lengths = vec![0usize; self.ops.len()];
        let mut open: Vec<usize> = Vec::with_capacity(MAX_DEPTH + 4);
        for (index, op) in self.ops.iter().enumerate().rev() {
            match op {
                Op::Close => open.push(0),
                Op::Bytes(bytes) => add_len(&mut open, bytes.len()),
                Op::Username => add_len(&mut open, username.len()),
                Op::Open(_) => {
                    let content = open.pop().unwrap_or(0);
                    lengths[index] = content;
                    add_len(&mut open, header_len(content) + content);
                }
            }
        }
        for (index, op) in self.ops.iter().enumerate() {
            match op {
                Op::Open(tag) => write_header(out, *tag, lengths[index]),
                Op::Bytes(bytes) => out.extend_from_slice(bytes),
                Op::Username => out.extend_from_slice(username),
                Op::Close => {}
            }
        }
    }

    /// Encoded length of the filter for a username of `username_len` bytes.
    pub fn encoded_len(&self, username_len: usize) -> usize {
        if let [Op::Bytes(bytes)] = self.ops.as_slice() {
            return bytes.len();
        }
        let mut open: Vec<usize> = Vec::with_capacity(MAX_DEPTH + 4);
        let mut total = 0usize;
        for op in self.ops.iter().rev() {
            match op {
                Op::Close => open.push(0),
                Op::Bytes(bytes) => add_to(&mut open, &mut total, bytes.len()),
                Op::Username => add_to(&mut open, &mut total, username_len),
                Op::Open(_) => {
                    let content = open.pop().unwrap_or(0);
                    add_to(&mut open, &mut total, header_len(content) + content);
                }
            }
        }
        total
    }
}

fn add_len(open: &mut [usize], len: usize) {
    if let Some(top) = open.last_mut() {
        *top += len;
    }
}

fn add_to(open: &mut [usize], total: &mut usize, len: usize) {
    match open.last_mut() {
        Some(top) => *top += len,
        None => *total += len,
    }
}

#[derive(Default)]
struct Builder {
    ops: Vec<Op>,
    slots: u8,
}

impl Builder {
    fn open(&mut self, tag: u8) {
        self.ops.push(Op::Open(tag));
    }

    fn close(&mut self) {
        self.ops.push(Op::Close);
    }

    fn bytes(&mut self, bytes: &[u8]) {
        if !bytes.is_empty() {
            self.ops.push(Op::Bytes(bytes.into()));
        }
    }

    /// `tag` element whose content is the raw bytes `bytes`.
    fn element(&mut self, tag: u8, bytes: &[u8]) {
        self.open(tag);
        self.bytes(bytes);
        self.close();
    }

    /// `tag` element whose content is a parsed assertion value.
    fn value(&mut self, tag: u8, raw: &[u8]) -> Result<(), FilterError> {
        self.open(tag);
        let mut literal: Vec<u8> = Vec::new();
        let mut index = 0;
        while index < raw.len() {
            if raw[index..].starts_with(PLACEHOLDER) {
                self.bytes(&literal);
                literal.clear();
                self.ops.push(Op::Username);
                self.slots = self.slots.saturating_add(1);
                index += PLACEHOLDER.len();
                continue;
            }
            match raw[index] {
                b'\\' => {
                    let high = raw.get(index + 1).and_then(|b| hex_value(*b));
                    let low = raw.get(index + 2).and_then(|b| hex_value(*b));
                    match (high, low) {
                        (Some(high), Some(low)) => literal.push((high << 4) | low),
                        _ => return Err(FilterError::InvalidEscape),
                    }
                    index += 3;
                }
                b'(' | b')' | b'*' | 0 => return Err(FilterError::InvalidEscape),
                byte => {
                    literal.push(byte);
                    index += 1;
                }
            }
        }
        self.bytes(&literal);
        self.close();
        Ok(())
    }
}

pub(crate) fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn parse(text: &[u8], out: &mut Builder) -> Result<(), FilterError> {
    // Tags of the open `&`, `|` and `!` filters.
    let mut stack = [0u8; MAX_DEPTH];
    let mut depth = 0usize;
    let mut pos = 0usize;
    loop {
        // At the start of a filter: "(".
        if text.get(pos) != Some(&b'(') {
            return Err(FilterError::Syntax);
        }
        pos += 1;
        match text.get(pos) {
            Some(&op @ (b'&' | b'|' | b'!')) => {
                if depth + 1 >= MAX_DEPTH {
                    return Err(FilterError::TooDeep);
                }
                let tag = match op {
                    b'&' => TAG_AND,
                    b'|' => TAG_OR,
                    _ => TAG_NOT,
                };
                stack[depth] = tag;
                depth += 1;
                out.open(tag);
                pos += 1;
                continue;
            }
            Some(_) => {
                let end = text[pos..]
                    .iter()
                    .position(|&byte| byte == b')')
                    .map(|offset| pos + offset)
                    .ok_or(FilterError::Syntax)?;
                parse_item(&text[pos..end], out)?;
                pos = end + 1;
            }
            None => return Err(FilterError::Syntax),
        }
        // A filter just ended; close every composite whose list ends here.
        loop {
            let Some(&tag) = depth.checked_sub(1).map(|top| &stack[top]) else {
                return if pos == text.len() {
                    Ok(())
                } else {
                    Err(FilterError::Syntax)
                };
            };
            if text.get(pos) != Some(&b')') {
                // `!` takes exactly one filter; `&`/`|` continue their list.
                if tag == TAG_NOT {
                    return Err(FilterError::Syntax);
                }
                break;
            }
            // Every composite reached here has at least one child: `(&)`
            // and `(|)` fail the "(" check at the top of the loop.
            out.close();
            depth -= 1;
            pos += 1;
        }
    }
}

/// Parse one `attr op value` item (the text between its parentheses).
fn parse_item(item: &[u8], out: &mut Builder) -> Result<(), FilterError> {
    if item.is_empty() {
        return Err(FilterError::Syntax);
    }
    let eq = item.iter().position(|&b| b == b'=').ok_or(FilterError::Syntax)?;
    let colon_before_eq = item[..eq].iter().position(|&b| b == b':');
    if let Some(colon) = colon_before_eq {
        return parse_extensible(item, colon, eq, out);
    }
    let (attr, tag) = match item[..eq].last() {
        Some(b'~') => (&item[..eq - 1], TAG_APPROX),
        Some(b'>') => (&item[..eq - 1], TAG_GREATER_OR_EQUAL),
        Some(b'<') => (&item[..eq - 1], TAG_LESS_OR_EQUAL),
        _ => (&item[..eq], TAG_EQUALITY),
    };
    check_attribute(attr)?;
    let value = &item[eq + 1..];
    if tag != TAG_EQUALITY {
        out.open(tag);
        out.element(TAG_OCTET_STRING, attr);
        out.value(TAG_OCTET_STRING, value)?;
        out.close();
        return Ok(());
    }
    if value == b"*" {
        out.element(TAG_PRESENT, attr);
        return Ok(());
    }
    if !value.contains(&b'*') {
        out.open(TAG_EQUALITY);
        out.element(TAG_OCTET_STRING, attr);
        out.value(TAG_OCTET_STRING, value)?;
        out.close();
        return Ok(());
    }
    // Substrings: initial? *(any)* final?, components split on '*'.
    let parts: Vec<&[u8]> = value.split(|&b| b == b'*').collect();
    let last = parts.len() - 1;
    out.open(TAG_SUBSTRINGS);
    out.element(TAG_OCTET_STRING, attr);
    out.open(TAG_SEQUENCE);
    for (index, part) in parts.iter().enumerate() {
        let tag = if index == 0 {
            TAG_SUB_INITIAL
        } else if index == last {
            TAG_SUB_FINAL
        } else {
            TAG_SUB_ANY
        };
        if part.is_empty() {
            if tag == TAG_SUB_ANY {
                return Err(FilterError::Syntax);
            }
            continue;
        }
        out.value(tag, part)?;
    }
    out.close();
    out.close();
    Ok(())
}

/// `attr [":dn"] [":" rule] ":=" value` or `[":dn"] ":" rule ":=" value`.
fn parse_extensible(item: &[u8], first_colon: usize, eq: usize, out: &mut Builder) -> Result<(), FilterError> {
    if eq == 0 || item[eq - 1] != b':' {
        return Err(FilterError::Syntax);
    }
    let attr = &item[..first_colon];
    let mut dn_attributes = false;
    let mut rule: Option<&[u8]> = None;
    // Text between the first colon and the `:=` colon: empty for `attr:=v`,
    // otherwise `:dn`, `:rule` or `:dn:rule`. Every segment must be non-empty.
    let tail = item.get(first_colon..eq - 1).unwrap_or(&[]);
    if !tail.is_empty() {
        let mut segments = tail[1..].split(|&b| b == b':');
        let mut next = segments.next();
        if next.is_some_and(|segment| segment.eq_ignore_ascii_case(b"dn")) {
            dn_attributes = true;
            next = segments.next();
        }
        if let Some(segment) = next {
            if segment.is_empty() {
                return Err(FilterError::Syntax);
            }
            if !is_numeric_oid(segment) {
                return Err(FilterError::InvalidAttribute);
            }
            rule = Some(segment);
        }
        if segments.next().is_some() {
            return Err(FilterError::Syntax);
        }
    }
    if attr.is_empty() && rule.is_none() {
        return Err(FilterError::Syntax);
    }
    if !attr.is_empty() {
        check_attribute(attr)?;
    }
    out.open(TAG_EXTENSIBLE);
    if let Some(rule) = rule {
        out.element(TAG_MRA_RULE, rule);
    }
    if !attr.is_empty() {
        out.element(TAG_MRA_TYPE, attr);
    }
    out.value(TAG_MRA_VALUE, &item[eq + 1..])?;
    if dn_attributes {
        out.bytes(&MRA_DN_ATTRIBUTES_TRUE);
    }
    out.close();
    Ok(())
}

fn check_attribute(attr: &[u8]) -> Result<(), FilterError> {
    if attr.windows(PLACEHOLDER.len()).any(|window| window == PLACEHOLDER) {
        return Err(FilterError::UsernameOutsideValue);
    }
    if is_attribute_description_bytes(attr) {
        Ok(())
    } else {
        Err(FilterError::InvalidAttribute)
    }
}

/// Whether `text` is an RFC 4512 attribute description: a `keystring` or a
/// numeric OID, optionally followed by `;option` segments.
pub fn is_attribute_description(text: &str) -> bool {
    is_attribute_description_bytes(text.as_bytes())
}

fn is_attribute_description_bytes(text: &[u8]) -> bool {
    let mut parts = text.split(|&b| b == b';');
    let Some(name) = parts.next() else { return false };
    let name_ok = is_keystring(name) || is_numeric_oid(name);
    name_ok && parts.all(|option| !option.is_empty() && option.iter().all(|&b| b.is_ascii_alphanumeric() || b == b'-'))
}

pub(crate) fn is_keystring(text: &[u8]) -> bool {
    matches!(text.first(), Some(b) if b.is_ascii_alphabetic())
        && text.iter().all(|&b| b.is_ascii_alphanumeric() || b == b'-')
}

pub(crate) fn is_numeric_oid(text: &[u8]) -> bool {
    !text.is_empty()
        && text.split(|&b| b == b'.').all(|number| {
            !number.is_empty() && number.iter().all(u8::is_ascii_digit) && (number.len() == 1 || number[0] != b'0')
        })
}

#[cfg(test)]
#[path = "../tests/unit/filter_test.rs"]
mod tests;
