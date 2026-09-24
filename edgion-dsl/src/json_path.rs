//! Definite JSONPath parsing shared by DSL compilation and bytecode validation.
//!
//! This intentionally implements only the bounded, unambiguous subset used by
//! `json_get`: `$`, dot members, quoted members, and non-negative indices.

use std::fmt;

/// Maximum UTF-8 byte length of an encoded JSON path.
pub const MAX_JSON_PATH_LEN: usize = 1_024;
/// Maximum number of member/index segments after the root.
pub const MAX_JSON_PATH_DEPTH: usize = 32;

/// One segment in a definite JSON path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JsonPathSegment {
    Member(String),
    Index(u64),
}

/// A parsed path that selects at most one structural location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefiniteJsonPath {
    segments: Vec<JsonPathSegment>,
}

impl DefiniteJsonPath {
    pub fn parse(input: &str) -> Result<Self, JsonPathError> {
        if input.len() > MAX_JSON_PATH_LEN {
            return Err(JsonPathError::new(
                0,
                format!(
                    "JSON path is too long: {} bytes (max {})",
                    input.len(),
                    MAX_JSON_PATH_LEN
                ),
            ));
        }
        if !input.starts_with('$') {
            return Err(JsonPathError::new(0, "JSON path must start with '$'"));
        }

        let bytes = input.as_bytes();
        let mut offset = 1usize;
        let mut segments = Vec::new();
        while offset < bytes.len() {
            if segments.len() == MAX_JSON_PATH_DEPTH {
                return Err(JsonPathError::new(
                    offset,
                    format!("JSON path exceeds maximum depth {}", MAX_JSON_PATH_DEPTH),
                ));
            }

            match bytes[offset] {
                b'.' => {
                    let start = offset + 1;
                    if start >= bytes.len() || !is_member_start(bytes[start]) {
                        return Err(JsonPathError::new(
                            start,
                            "dot member must start with an ASCII letter or '_'",
                        ));
                    }
                    offset = start + 1;
                    while offset < bytes.len() && is_member_continue(bytes[offset]) {
                        offset += 1;
                    }
                    let member = input[start..offset].to_string();
                    segments.push(JsonPathSegment::Member(member));
                }
                b'[' => {
                    let content = offset + 1;
                    if content >= bytes.len() {
                        return Err(JsonPathError::new(offset, "unterminated '[' segment"));
                    }
                    if bytes[content] == b'"' {
                        let (member, next) = parse_quoted_member(input, content)?;
                        segments.push(JsonPathSegment::Member(member));
                        offset = next;
                    } else {
                        let (index, next) = parse_index(input, content)?;
                        segments.push(JsonPathSegment::Index(index));
                        offset = next;
                    }
                }
                _ => {
                    return Err(JsonPathError::new(offset, "expected '.', '[\"member\"]', or '[index]'"));
                }
            }
        }
        Ok(Self { segments })
    }

    pub fn segments(&self) -> &[JsonPathSegment] {
        &self.segments
    }
}

impl std::str::FromStr for DefiniteJsonPath {
    type Err = JsonPathError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

/// Parse error with a byte offset into the encoded path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonPathError {
    offset: usize,
    message: String,
}

impl JsonPathError {
    fn new(offset: usize, message: impl Into<String>) -> Self {
        Self {
            offset,
            message: message.into(),
        }
    }

    pub fn offset(&self) -> usize {
        self.offset
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for JsonPathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} at byte {}", self.message, self.offset)
    }
}

impl std::error::Error for JsonPathError {}

fn is_member_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_'
}

fn is_member_continue(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn parse_quoted_member(input: &str, quote_start: usize) -> Result<(String, usize), JsonPathError> {
    let bytes = input.as_bytes();
    let mut offset = quote_start + 1;
    let mut escaped = false;
    let quote_end = loop {
        let Some(&byte) = bytes.get(offset) else {
            return Err(JsonPathError::new(quote_start, "unterminated quoted member"));
        };
        if escaped {
            escaped = false;
        } else if byte == b'\\' {
            escaped = true;
        } else if byte == b'"' {
            break offset;
        }
        offset += 1;
    };

    let close_bracket = quote_end + 1;
    if bytes.get(close_bracket) != Some(&b']') {
        return Err(JsonPathError::new(close_bracket, "quoted member must end with ']'"));
    }
    let encoded = &input[quote_start..=quote_end];
    let member = serde_json::from_str::<String>(encoded)
        .map_err(|error| JsonPathError::new(quote_start, format!("invalid quoted JSON member: {}", error)))?;
    Ok((member, close_bracket + 1))
}

fn parse_index(input: &str, start: usize) -> Result<(u64, usize), JsonPathError> {
    let bytes = input.as_bytes();
    if !bytes[start].is_ascii_digit() {
        return Err(JsonPathError::new(
            start,
            "array index must be a non-negative decimal integer",
        ));
    }
    let mut offset = start + 1;
    while offset < bytes.len() && bytes[offset].is_ascii_digit() {
        offset += 1;
    }
    if bytes.get(offset) != Some(&b']') {
        return Err(JsonPathError::new(offset, "array index must end with ']'"));
    }
    let index = input[start..offset]
        .parse::<u64>()
        .map_err(|_| JsonPathError::new(start, "array index exceeds u64"))?;
    Ok((index, offset + 1))
}

#[cfg(test)]
#[path = "../tests/unit/json_path_test.rs"]
mod tests;
