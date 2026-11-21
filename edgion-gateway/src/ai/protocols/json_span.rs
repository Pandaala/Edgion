//! Bounded, offset-reporting JSON parsing shared by the canonical AI paths.
//!
//! Produces a small owned tree in which every string carries the byte range of
//! its complete quoted JSON token, so a caller can address the exact bytes to
//! rewrite without re-parsing.
//!
//! Two decode modes exist. The canonical SSE observer decodes every string
//! eagerly, which is what it has always done. The request-side prompt guard
//! parses with decoding disabled and decodes only the tokens it actually
//! selects, so peak allocation follows the guard's own span bounds rather than
//! the size of the request body.

use std::borrow::Cow;

use serde::Deserialize;

pub(crate) const MAX_JSON_DEPTH: usize = 64;
pub(crate) const MAX_JSON_NODES: usize = 16_384;

#[derive(Debug)]
pub(crate) enum Node {
    Object(Vec<(String, Node)>),
    Array(Vec<Node>),
    String(StringToken),
    Number(String),
    Null,
    Boolean,
}

#[derive(Debug)]
pub(crate) struct StringToken {
    pub(crate) value: String,
    pub(crate) start: usize,
    pub(crate) end: usize,
}

impl Node {
    pub(crate) fn object_marker(&self) -> Option<&Self> {
        matches!(self, Self::Object(_)).then_some(self)
    }
    pub(crate) fn array(&self) -> Option<&[Self]> {
        if let Self::Array(v) = self {
            Some(v)
        } else {
            None
        }
    }
    pub(crate) fn string_token(&self) -> Option<&StringToken> {
        if let Self::String(v) = self {
            Some(v)
        } else {
            None
        }
    }
    pub(crate) fn u64(&self) -> Option<u64> {
        if let Self::Number(v) = self {
            v.parse().ok()
        } else {
            None
        }
    }
    pub(crate) fn u32(&self) -> Option<u32> {
        self.u64().and_then(|v| u32::try_from(v).ok())
    }
}

pub(crate) fn fields(node: &Node) -> Option<&[(String, Node)]> {
    if let Node::Object(fields) = node {
        Some(fields)
    } else {
        None
    }
}

pub(crate) enum Field<'a> {
    Missing,
    Unique(&'a Node),
    Ambiguous,
}

pub(crate) fn field<'a>(node: &'a Node, name: &str) -> Field<'a> {
    let Some(fields) = fields(node) else {
        return Field::Ambiguous;
    };
    let mut found = fields.iter().filter(|(key, _)| key == name).map(|(_, value)| value);
    match (found.next(), found.next()) {
        (None, _) => Field::Missing,
        (Some(value), None) => Field::Unique(value),
        (Some(_), Some(_)) => Field::Ambiguous,
    }
}

pub(crate) fn last_field<'a>(node: &'a Node, name: &str) -> Option<&'a Node> {
    fields(node)?
        .iter()
        .rev()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value)
}

pub(crate) struct Parser<'a> {
    input: &'a [u8],
    offset: usize,
    nodes: usize,
    decode_strings: bool,
}

impl<'a> Parser<'a> {
    pub(crate) fn new(input: &'a [u8]) -> Self {
        Self {
            input,
            offset: 0,
            nodes: 0,
            decode_strings: true,
        }
    }

    /// Parse recording string-token offsets without decoding their contents.
    ///
    /// `StringToken::value` is left empty; the caller decodes the tokens it
    /// selects from the original input.
    pub(crate) fn new_without_decoding(input: &'a [u8]) -> Self {
        Self {
            input,
            offset: 0,
            nodes: 0,
            decode_strings: false,
        }
    }

    pub(crate) fn parse(mut self) -> Result<Node, ()> {
        let value = self.value(0)?;
        self.ws();
        (self.offset == self.input.len()).then_some(value).ok_or(())
    }

    fn value(&mut self, depth: usize) -> Result<Node, ()> {
        if depth > MAX_JSON_DEPTH || self.nodes >= MAX_JSON_NODES {
            return Err(());
        }
        self.nodes += 1;
        self.ws();
        match self.input.get(self.offset).copied() {
            Some(b'{') => self.object(depth + 1),
            Some(b'[') => self.array(depth + 1),
            Some(b'"') => self.string().map(Node::String),
            Some(b'-' | b'0'..=b'9') => self.number().map(Node::Number),
            Some(b't') => {
                self.literal(b"true")?;
                Ok(Node::Boolean)
            }
            Some(b'f') => {
                self.literal(b"false")?;
                Ok(Node::Boolean)
            }
            Some(b'n') => {
                self.literal(b"null")?;
                Ok(Node::Null)
            }
            _ => Err(()),
        }
    }

    fn object(&mut self, depth: usize) -> Result<Node, ()> {
        self.offset += 1;
        let mut values = Vec::new();
        self.ws();
        if self.take(b'}') {
            return Ok(Node::Object(values));
        }
        loop {
            // Object keys are always decoded, even in offset-only mode: field
            // lookup is by name, so an undecoded key would make every lookup
            // miss. Only values honour the decode mode.
            let token = self.string()?;
            let key = if !self.decode_strings {
                decode_json_string(&self.input[token.start..token.end]).ok_or(())?
            } else {
                token.value
            };
            self.ws();
            if !self.take(b':') {
                return Err(());
            }
            values.push((key, self.value(depth)?));
            self.ws();
            if self.take(b'}') {
                break;
            }
            if !self.take(b',') {
                return Err(());
            }
            self.ws();
        }
        Ok(Node::Object(values))
    }

    fn array(&mut self, depth: usize) -> Result<Node, ()> {
        self.offset += 1;
        let mut values = Vec::new();
        self.ws();
        if self.take(b']') {
            return Ok(Node::Array(values));
        }
        loop {
            values.push(self.value(depth)?);
            self.ws();
            if self.take(b']') {
                break;
            }
            if !self.take(b',') {
                return Err(());
            }
        }
        Ok(Node::Array(values))
    }

    fn string(&mut self) -> Result<StringToken, ()> {
        let start = self.offset;
        if !self.take(b'"') {
            return Err(());
        }
        let mut escaped = false;
        while let Some(byte) = self.input.get(self.offset).copied() {
            self.offset += 1;
            if escaped {
                escaped = false;
                continue;
            }
            match byte {
                b'\\' => escaped = true,
                b'"' => {
                    let end = self.offset;
                    let value = if self.decode_strings {
                        decode_json_string(&self.input[start..end]).ok_or(())?
                    } else {
                        String::new()
                    };
                    return Ok(StringToken { value, start, end });
                }
                0..=0x1f => return Err(()),
                _ => {}
            }
        }
        Err(())
    }

    fn number(&mut self) -> Result<String, ()> {
        let start = self.offset;
        while self
            .input
            .get(self.offset)
            .is_some_and(|byte| matches!(byte, b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9'))
        {
            self.offset += 1;
        }
        let text = std::str::from_utf8(&self.input[start..self.offset]).map_err(|_| ())?;
        serde_json::from_str::<serde_json::Number>(text).map_err(|_| ())?;
        Ok(text.to_owned())
    }

    fn literal(&mut self, literal: &[u8]) -> Result<(), ()> {
        if self.input.get(self.offset..self.offset + literal.len()) == Some(literal) {
            self.offset += literal.len();
            Ok(())
        } else {
            Err(())
        }
    }

    fn ws(&mut self) {
        while self
            .input
            .get(self.offset)
            .is_some_and(|byte| matches!(byte, b' ' | b'\t' | b'\n' | b'\r'))
        {
            self.offset += 1;
        }
    }
    fn take(&mut self, byte: u8) -> bool {
        if self.input.get(self.offset) == Some(&byte) {
            self.offset += 1;
            true
        } else {
            false
        }
    }
}

/// Decode one complete quoted JSON string token into its logical value.
pub(crate) fn decode_json_string(token: &[u8]) -> Option<String> {
    serde_json::from_slice::<String>(token).ok()
}

#[derive(Deserialize)]
#[serde(transparent)]
struct BorrowedJsonString<'a>(#[serde(borrow)] Cow<'a, str>);

/// Decode a complete quoted token, borrowing its text when no escapes occur.
pub(crate) fn decode_json_string_borrowed(token: &[u8]) -> Option<Cow<'_, str>> {
    serde_json::from_slice::<BorrowedJsonString<'_>>(token)
        .ok()
        .map(|value| value.0)
}

#[cfg(test)]
#[path = "../../../tests/unit/ai/protocols/json_span_test.rs"]
mod tests;
