//! Immutable XML 1.0 subset with producer-side limits and execution work charging.
//!
//! The tokenizer is allocation-free. This module owns structural validation,
//! namespace identity, character decoding and bounded construction/serialization.

use std::fmt;
use std::sync::Arc;

use xmlparser::{ElementEnd, Token, Tokenizer, XmlCharExt};

use crate::error::RuntimeError;
use crate::value::ValueWorkBudget;

pub const MAX_XML_BYTES: usize = 1 << 20;
pub const MAX_XML_DEPTH: usize = 32;
pub const MAX_XML_NODES: usize = 4096;
pub const MAX_XML_ATTRIBUTES: usize = 4096;
pub const MAX_XML_TAG_ATTRIBUTES: usize = 64;
pub const MAX_XML_TAG_NAMESPACES: usize = 64;
pub const MAX_XML_NAME_BYTES: usize = 256;
pub const MAX_XML_NAMESPACE_BYTES: usize = 1024;
const XML_URI: &str = "http://www.w3.org/XML/1998/namespace";
const XMLNS_URI: &str = "http://www.w3.org/2000/xmlns/";
const XINCLUDE_URI: &str = "http://www.w3.org/2001/XInclude";
const NODE_UNITS: u64 = 512;

#[derive(Debug, Clone)]
pub enum XmlError {
    Invalid,
    Allocation,
    Limit { kind: &'static str, limit: usize },
    Work(RuntimeError),
}

impl From<RuntimeError> for XmlError {
    fn from(error: RuntimeError) -> Self {
        Self::Work(error)
    }
}

fn bound(len: usize, limit: usize, kind: &'static str) -> Result<(), XmlError> {
    if len > limit {
        Err(XmlError::Limit { kind, limit })
    } else {
        Ok(())
    }
}

#[derive(Clone, Copy, Default)]
struct Summary {
    nodes: usize,
    attributes: usize,
    depth: usize,
    payload: usize,
}

impl Summary {
    fn check(self) -> Result<(), XmlError> {
        bound(self.nodes, MAX_XML_NODES, "nodes")?;
        bound(self.attributes, MAX_XML_ATTRIBUTES, "attributes")?;
        bound(self.depth, MAX_XML_DEPTH, "depth")?;
        bound(self.payload, MAX_XML_BYTES, "decoded bytes")
    }
}

enum Kind {
    Element {
        namespace: Arc<str>,
        name: Arc<str>,
        attributes: Vec<XmlValue>,
        children: Vec<XmlValue>,
    },
    Attribute {
        namespace: Arc<str>,
        name: Arc<str>,
        value: Arc<str>,
    },
    Text(Arc<str>),
}

struct Node {
    kind: Kind,
    summary: Summary,
}

/// Opaque immutable node. Equality is identity, not an uncharged deep comparison.
#[derive(Clone)]
pub struct XmlValue(Arc<Node>);

impl fmt::Debug for XmlValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[xml]")
    }
}
impl PartialEq for XmlValue {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for XmlValue {}

fn ncname(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c != ':' && c.is_xml_name_start()) && chars.all(|c| c != ':' && c.is_xml_name())
}

fn owned(value: &str, limit: usize, kind: &'static str, budget: &mut ValueWorkBudget) -> Result<Arc<str>, XmlError> {
    bound(value.len(), limit, kind)?;
    budget.charge((value.len() as u64).saturating_mul(4))?;
    if !value.chars().all(|c| c.is_xml_char()) {
        return Err(XmlError::Invalid);
    }
    Ok(Arc::from(value))
}

fn expanded_name(namespace: &str, name: &str, budget: &mut ValueWorkBudget) -> Result<(Arc<str>, Arc<str>), XmlError> {
    let name = owned(name, MAX_XML_NAME_BYTES, "name bytes", budget)?;
    let namespace = owned(namespace, MAX_XML_NAMESPACE_BYTES, "namespace bytes", budget)?;
    if !ncname(&name)
        || name.as_ref() == "xmlns"
        || namespace.as_ref() == XMLNS_URI
        || namespace.as_ref() == XINCLUDE_URI
    {
        return Err(XmlError::Invalid);
    }
    Ok((namespace, name))
}

fn same(a: &str, b: &str, budget: &mut ValueWorkBudget) -> Result<bool, XmlError> {
    budget.charge(8 + a.len() as u64 + b.len() as u64)?;
    Ok(a == b)
}

impl XmlValue {
    pub fn kind(&self) -> &'static str {
        match self.0.kind {
            Kind::Element { .. } => "element",
            Kind::Attribute { .. } => "attribute",
            Kind::Text(_) => "text",
        }
    }

    pub fn name(&self) -> Option<(&str, &str)> {
        match &self.0.kind {
            Kind::Element { namespace, name, .. } | Kind::Attribute { namespace, name, .. } => Some((namespace, name)),
            Kind::Text(_) => None,
        }
    }

    pub fn children(&self) -> Option<&[Self]> {
        match &self.0.kind {
            Kind::Element { children, .. } => Some(children),
            _ => None,
        }
    }

    pub fn attributes(&self) -> Option<&[Self]> {
        match &self.0.kind {
            Kind::Element { attributes, .. } => Some(attributes),
            _ => None,
        }
    }

    pub fn text(value: &str, budget: &mut ValueWorkBudget) -> Result<Self, XmlError> {
        budget.charge(NODE_UNITS)?;
        let value = owned(value, MAX_XML_BYTES, "decoded bytes", budget)?;
        let summary = Summary {
            nodes: 1,
            payload: value.len(),
            ..Summary::default()
        };
        Ok(Self(Arc::new(Node {
            kind: Kind::Text(value),
            summary,
        })))
    }

    pub fn attribute(namespace: &str, name: &str, value: &str, budget: &mut ValueWorkBudget) -> Result<Self, XmlError> {
        budget.charge(NODE_UNITS)?;
        let (namespace, name) = expanded_name(namespace, name, budget)?;
        let value = owned(value, MAX_XML_BYTES, "decoded bytes", budget)?;
        let summary = Summary {
            attributes: 1,
            payload: namespace.len() + name.len() + value.len(),
            ..Summary::default()
        };
        summary.check()?;
        Ok(Self(Arc::new(Node {
            kind: Kind::Attribute { namespace, name, value },
            summary,
        })))
    }

    pub fn element(
        namespace: &str,
        name: &str,
        attributes: Vec<Self>,
        children: Vec<Self>,
        budget: &mut ValueWorkBudget,
    ) -> Result<Self, XmlError> {
        bound(attributes.len(), MAX_XML_TAG_ATTRIBUTES, "attributes per element")?;
        bound(children.len(), MAX_XML_NODES, "nodes")?;
        budget.charge(NODE_UNITS)?;
        let (namespace, name) = expanded_name(namespace, name, budget)?;
        let mut summary = Summary {
            nodes: 1,
            depth: 1,
            payload: namespace.len() + name.len(),
            ..Summary::default()
        };
        for (index, attr) in attributes.iter().enumerate() {
            budget.charge(32)?;
            let Kind::Attribute { namespace, name, .. } = &attr.0.kind else {
                return Err(XmlError::Invalid);
            };
            for previous in &attributes[..index] {
                let Some((previous_ns, previous_name)) = previous.name() else {
                    return Err(XmlError::Invalid);
                };
                if same(namespace, previous_ns, budget)? && same(name, previous_name, budget)? {
                    return Err(XmlError::Invalid);
                }
            }
            summary.attributes += 1;
            summary.payload = summary.payload.saturating_add(attr.0.summary.payload);
            summary.check()?;
        }
        for child in &children {
            budget.charge(32)?;
            if matches!(child.0.kind, Kind::Attribute { .. }) {
                return Err(XmlError::Invalid);
            }
            let child_summary = child.0.summary;
            summary.nodes = summary.nodes.saturating_add(child_summary.nodes);
            summary.attributes = summary.attributes.saturating_add(child_summary.attributes);
            summary.depth = summary.depth.max(1 + child_summary.depth);
            summary.payload = summary.payload.saturating_add(child_summary.payload);
            summary.check()?;
        }
        Ok(Self(Arc::new(Node {
            kind: Kind::Element {
                namespace,
                name,
                attributes,
                children,
            },
            summary,
        })))
    }

    pub fn find(
        &self,
        namespace: &str,
        name: &str,
        mut occurrence: usize,
        budget: &mut ValueWorkBudget,
    ) -> Result<Option<Self>, XmlError> {
        for child in self.children().ok_or(XmlError::Invalid)? {
            budget.charge(16)?;
            if let Some((ns, local)) = child.name() {
                if same(ns, namespace, budget)? && same(local, name, budget)? {
                    if occurrence == 0 {
                        return Ok(Some(child.clone()));
                    }
                    occurrence -= 1;
                }
            }
        }
        Ok(None)
    }

    pub fn attr_value(
        &self,
        namespace: &str,
        name: &str,
        budget: &mut ValueWorkBudget,
    ) -> Result<Option<&str>, XmlError> {
        for attr in self.attributes().ok_or(XmlError::Invalid)? {
            budget.charge(16)?;
            if let Kind::Attribute {
                namespace: ns,
                name: local,
                value,
            } = &attr.0.kind
            {
                if same(ns, namespace, budget)? && same(local, name, budget)? {
                    return Ok(Some(value));
                }
            }
        }
        Ok(None)
    }

    /// Concatenated semantic text; attributes return their own value.
    pub fn value(&self, limit: usize, budget: &mut ValueWorkBudget) -> Result<String, XmlError> {
        fn visit(node: &XmlValue, out: &mut Writer<'_>) -> Result<(), XmlError> {
            out.budget.charge(16)?;
            match &node.0.kind {
                Kind::Text(text) | Kind::Attribute { value: text, .. } => out.append(text),
                Kind::Element { children, .. } => {
                    for child in children {
                        visit(child, out)?;
                    }
                    Ok(())
                }
            }
        }
        let mut writer = Writer::new(limit.min(MAX_XML_BYTES), budget);
        visit(self, &mut writer)?;
        // Includes the downstream String -> Arc conversion overlap.
        writer.budget.charge(writer.output.len() as u64)?;
        Ok(writer.output)
    }

    pub fn stringify(&self, limit: usize, budget: &mut ValueWorkBudget) -> Result<String, XmlError> {
        if !matches!(self.0.kind, Kind::Element { .. }) {
            return Err(XmlError::Invalid);
        }
        let mut writer = Writer::new(limit.min(MAX_XML_BYTES), budget);
        serialize(self, &mut writer)?;
        Ok(writer.output)
    }
}

struct Writer<'a> {
    output: String,
    limit: usize,
    budget: &'a mut ValueWorkBudget,
}
impl<'a> Writer<'a> {
    fn new(limit: usize, budget: &'a mut ValueWorkBudget) -> Self {
        Self {
            output: String::new(),
            limit,
            budget,
        }
    }
    fn append(&mut self, text: &str) -> Result<(), XmlError> {
        let next = self.output.len().saturating_add(text.len());
        bound(next, self.limit, "output bytes")?;
        self.budget.charge((text.len() as u64).saturating_mul(2))?;
        if next > self.output.capacity() {
            let target = next.max(self.output.capacity().saturating_mul(2)).min(self.limit);
            self.output
                .try_reserve_exact(target - self.output.len())
                .map_err(|_| XmlError::Allocation)?;
        }
        self.output.push_str(text);
        Ok(())
    }
    fn escaped(&mut self, text: &str, attribute: bool) -> Result<(), XmlError> {
        for c in text.chars() {
            self.budget.charge(c.len_utf8() as u64)?;
            let mut bytes = [0u8; 4];
            self.append(match c {
                '&' => "&amp;",
                '<' => "&lt;",
                '>' => "&gt;",
                '"' if attribute => "&quot;",
                '\r' => "&#13;",
                '\n' if attribute => "&#10;",
                '\t' if attribute => "&#9;",
                _ => c.encode_utf8(&mut bytes),
            })?;
        }
        Ok(())
    }
}

fn serialize(node: &XmlValue, out: &mut Writer<'_>) -> Result<(), XmlError> {
    out.budget.charge(16)?;
    match &node.0.kind {
        Kind::Text(text) => out.escaped(text, false),
        Kind::Attribute { .. } => Err(XmlError::Invalid),
        Kind::Element {
            namespace,
            name,
            attributes,
            children,
        } => {
            let prefix = if namespace.is_empty() {
                ""
            } else if namespace.as_ref() == XML_URI {
                "xml:"
            } else {
                "n:"
            };
            out.budget.charge(64 * (attributes.len() as u64 + 1))?;
            let mut bindings: Vec<(&str, Option<usize>)> = Vec::new();
            bindings
                .try_reserve_exact(attributes.len() + 1)
                .map_err(|_| XmlError::Allocation)?;
            if prefix == "n:" {
                bindings.push((namespace.as_ref(), None));
            }
            out.append("<")?;
            out.append(prefix)?;
            out.append(name)?;
            if !namespace.is_empty() && namespace.as_ref() != XML_URI {
                out.append(" xmlns:n=\"")?;
                out.escaped(namespace, true)?;
                out.append("\"")?;
            }
            for (index, attr) in attributes.iter().enumerate() {
                let Kind::Attribute { namespace, name, value } = &attr.0.kind else {
                    return Err(XmlError::Invalid);
                };
                let mut declare = false;
                let prefix = if namespace.is_empty() {
                    String::new()
                } else if namespace.as_ref() == XML_URI {
                    "xml:".to_string()
                } else {
                    let mut binding = None;
                    for (uri, prefix_index) in &bindings {
                        if same(uri, namespace, out.budget)? {
                            binding = Some(*prefix_index);
                            break;
                        }
                    }
                    let prefix_index = if let Some(prefix_index) = binding {
                        prefix_index
                    } else {
                        bound(bindings.len() + 1, MAX_XML_TAG_NAMESPACES, "namespaces per element")?;
                        bindings.push((namespace.as_ref(), Some(index)));
                        declare = true;
                        Some(index)
                    };
                    match prefix_index {
                        Some(index) => format!("a{index}:"),
                        None => "n:".to_string(),
                    }
                };
                if declare {
                    out.append(" xmlns:")?;
                    out.append(prefix.trim_end_matches(':'))?;
                    out.append("=\"")?;
                    out.escaped(namespace, true)?;
                    out.append("\"")?;
                }
                out.append(" ")?;
                out.append(&prefix)?;
                out.append(name)?;
                out.append("=\"")?;
                out.escaped(value, true)?;
                out.append("\"")?;
            }
            if children.is_empty() {
                return out.append("/>");
            }
            out.append(">")?;
            for child in children {
                serialize(child, out)?;
            }
            out.append("</")?;
            out.append(prefix)?;
            out.append(name)?;
            out.append(">")
        }
    }
}

/// XML whitespace normalization occurs before reference decoding.
fn decode(
    raw: &str,
    attribute: bool,
    references: bool,
    limit: usize,
    budget: &mut ValueWorkBudget,
) -> Result<String, XmlError> {
    let mut writer = Writer::new(limit, budget);
    let mut chars = raw.chars();
    while let Some(mut c) = chars.next() {
        let mut from_reference = false;
        if c == '&' && references {
            let start = chars.as_str();
            let mut len = 0;
            let mut byte_len = 0;
            loop {
                match chars.next() {
                    Some(';') => break,
                    Some(part) if len < 32 => {
                        len += 1;
                        byte_len += part.len_utf8();
                    }
                    _ => return Err(XmlError::Invalid),
                }
            }
            let entity = &start[..byte_len];
            c = match entity {
                "amp" => '&',
                "lt" => '<',
                "gt" => '>',
                "quot" => '"',
                "apos" => '\'',
                numeric
                    if numeric.starts_with("#x")
                        && !numeric[2..].is_empty()
                        && numeric[2..].bytes().all(|c| c.is_ascii_hexdigit()) =>
                {
                    u32::from_str_radix(&numeric[2..], 16)
                        .ok()
                        .and_then(char::from_u32)
                        .ok_or(XmlError::Invalid)?
                }
                numeric
                    if numeric.starts_with('#')
                        && !numeric[1..].is_empty()
                        && numeric[1..].bytes().all(|c| c.is_ascii_digit()) =>
                {
                    numeric[1..]
                        .parse::<u32>()
                        .ok()
                        .and_then(char::from_u32)
                        .ok_or(XmlError::Invalid)?
                }
                _ => return Err(XmlError::Invalid),
            };
            if !c.is_xml_char() {
                return Err(XmlError::Invalid);
            }
            from_reference = true;
        }
        if !from_reference {
            if c == '\r' {
                if chars.clone().next() == Some('\n') {
                    chars.next();
                }
                c = '\n';
            }
            if attribute && matches!(c, '\n' | '\t') {
                c = ' ';
            }
        }
        let mut bytes = [0u8; 4];
        let decoded = c.encode_utf8(&mut bytes);
        // Two emitted units cover String capacity; two more cover Arc conversion.
        writer.budget.charge((decoded.len() as u64).saturating_mul(2))?;
        writer.append(decoded)?;
    }
    Ok(writer.output)
}

struct RawAttribute<'a> {
    prefix: &'a str,
    name: &'a str,
    value: &'a str,
}
struct Pending<'a> {
    prefix: &'a str,
    name: &'a str,
    attrs: Vec<RawAttribute<'a>>,
    ordinary_count: usize,
    namespace_count: usize,
}
struct Frame<'a> {
    prefix: &'a str,
    name: &'a str,
    namespace: Arc<str>,
    bindings: Vec<(&'a str, Arc<str>)>,
    attributes: Vec<XmlValue>,
    children: Vec<XmlValue>,
}

fn resolve<'a>(
    prefix: &str,
    bindings: &'a [(&str, Arc<str>)],
    frames: &'a [Frame<'_>],
    budget: &mut ValueWorkBudget,
) -> Result<Arc<str>, XmlError> {
    if prefix == "xml" {
        return Ok(Arc::from(XML_URI));
    }
    for (key, uri) in bindings
        .iter()
        .rev()
        .chain(frames.iter().rev().flat_map(|frame| frame.bindings.iter().rev()))
    {
        if same(key, prefix, budget)? {
            return Ok(Arc::clone(uri));
        }
    }
    if prefix.is_empty() {
        Ok(Arc::from(""))
    } else {
        Err(XmlError::Invalid)
    }
}

fn finish_frame(frame: Frame<'_>, budget: &mut ValueWorkBudget) -> Result<XmlValue, XmlError> {
    XmlValue::element(&frame.namespace, frame.name, frame.attributes, frame.children, budget)
}

fn append_node(
    node: XmlValue,
    frames: &mut [Frame<'_>],
    root: &mut Option<XmlValue>,
    budget: &mut ValueWorkBudget,
) -> Result<(), XmlError> {
    budget.charge(32)?;
    if let Some(frame) = frames.last_mut() {
        bound(frame.children.len() + 1, MAX_XML_NODES, "nodes")?;
        frame.children.push(node);
    } else if root.is_none() && node.kind() == "element" {
        *root = Some(node);
    } else {
        return Err(XmlError::Invalid);
    }
    Ok(())
}

/// Parse one complete XML 1.0 document. No resolver, DTD or include support exists.
pub fn parse(source: &str, budget: &mut ValueWorkBudget) -> Result<XmlValue, XmlError> {
    bound(source.len(), MAX_XML_BYTES, "source bytes")?;
    budget.charge((source.len() as u64).saturating_mul(4))?;
    if !source.chars().all(|c| c.is_xml_char()) {
        return Err(XmlError::Invalid);
    }
    let mut frames: Vec<Frame<'_>> = Vec::new();
    let mut pending: Option<Pending<'_>> = None;
    let mut root = None;
    let mut nodes = 0;
    let mut attributes = 0;
    let mut tokenizer = Tokenizer::from(source);
    loop {
        // Lexical work is prepaid for the complete bounded source.
        budget.charge(16)?;
        let Some(token) = tokenizer.next() else {
            break;
        };
        let token = token.map_err(|_| XmlError::Invalid)?;
        match token {
            Token::Declaration { version, encoding, .. } => {
                if version.as_str() != "1.0" || encoding.is_some_and(|v| !v.as_str().eq_ignore_ascii_case("utf-8")) {
                    return Err(XmlError::Invalid);
                }
            }
            Token::DtdStart { .. }
            | Token::EmptyDtd { .. }
            | Token::EntityDeclaration { .. }
            | Token::DtdEnd { .. } => return Err(XmlError::Invalid),
            Token::ProcessingInstruction { target, span, .. } => {
                bound(target.as_str().len(), MAX_XML_NAME_BYTES, "name bytes")?;
                if target.as_str().eq_ignore_ascii_case("xml") {
                    return Err(XmlError::Invalid);
                }
                let tail = &span.as_str().as_bytes()[2 + target.as_str().len()..];
                if !tail.starts_with(b"?>") && !matches!(tail.first(), Some(b' ' | b'\t' | b'\r' | b'\n')) {
                    return Err(XmlError::Invalid);
                }
            }
            Token::Comment { .. } => {}
            Token::ElementStart { prefix, local, .. } => {
                if pending.is_some() || (frames.is_empty() && root.is_some()) {
                    return Err(XmlError::Invalid);
                }
                bound(frames.len() + 1, MAX_XML_DEPTH, "depth")?;
                bound(prefix.as_str().len(), MAX_XML_NAME_BYTES, "name bytes")?;
                bound(local.as_str().len(), MAX_XML_NAME_BYTES, "name bytes")?;
                if !ncname(local.as_str()) || (!prefix.as_str().is_empty() && !ncname(prefix.as_str())) {
                    return Err(XmlError::Invalid);
                }
                nodes += 1;
                bound(nodes, MAX_XML_NODES, "nodes")?;
                budget.charge(NODE_UNITS)?;
                pending = Some(Pending {
                    prefix: prefix.as_str(),
                    name: local.as_str(),
                    attrs: Vec::new(),
                    ordinary_count: 0,
                    namespace_count: 0,
                });
            }
            Token::Attribute {
                prefix, local, value, ..
            } => {
                let tag = pending.as_mut().ok_or(XmlError::Invalid)?;
                let is_namespace =
                    prefix.as_str() == "xmlns" || (prefix.as_str().is_empty() && local.as_str() == "xmlns");
                let count = if is_namespace {
                    &mut tag.namespace_count
                } else {
                    &mut tag.ordinary_count
                };
                *count += 1;
                bound(
                    *count,
                    if is_namespace {
                        MAX_XML_TAG_NAMESPACES
                    } else {
                        MAX_XML_TAG_ATTRIBUTES
                    },
                    if is_namespace {
                        "namespaces per element"
                    } else {
                        "attributes per element"
                    },
                )?;
                bound(prefix.as_str().len(), MAX_XML_NAME_BYTES, "name bytes")?;
                bound(local.as_str().len(), MAX_XML_NAME_BYTES, "name bytes")?;
                if !ncname(local.as_str()) || (!prefix.as_str().is_empty() && !ncname(prefix.as_str())) {
                    return Err(XmlError::Invalid);
                }
                for previous in &tag.attrs {
                    if same(previous.prefix, prefix.as_str(), budget)? && same(previous.name, local.as_str(), budget)? {
                        return Err(XmlError::Invalid);
                    }
                }
                budget.charge(NODE_UNITS)?;
                tag.attrs.push(RawAttribute {
                    prefix: prefix.as_str(),
                    name: local.as_str(),
                    value: value.as_str(),
                });
            }
            Token::ElementEnd {
                end: end @ (ElementEnd::Open | ElementEnd::Empty),
                ..
            } => {
                let tag = pending.take().ok_or(XmlError::Invalid)?;
                let mut bindings = Vec::new();
                for attr in &tag.attrs {
                    if attr.prefix == "xmlns" || (attr.prefix.is_empty() && attr.name == "xmlns") {
                        let key = if attr.prefix.is_empty() { "" } else { attr.name };
                        let uri = decode(attr.value, true, true, MAX_XML_NAMESPACE_BYTES, budget)?;
                        bound(uri.len(), MAX_XML_NAMESPACE_BYTES, "namespace bytes")?;
                        if key == "xmlns"
                            || uri == XMLNS_URI
                            || (key == "xml") != (uri == XML_URI)
                            || (!key.is_empty() && uri.is_empty())
                        {
                            return Err(XmlError::Invalid);
                        }
                        budget.charge(NODE_UNITS + uri.len() as u64)?;
                        bindings.push((key, Arc::from(uri)));
                    }
                }
                let namespace = resolve(tag.prefix, &bindings, &frames, budget)?;
                let mut attrs = Vec::new();
                for attr in &tag.attrs {
                    if attr.prefix == "xmlns" || (attr.prefix.is_empty() && attr.name == "xmlns") {
                        continue;
                    }
                    attributes += 1;
                    bound(attributes, MAX_XML_ATTRIBUTES, "attributes")?;
                    let namespace = if attr.prefix.is_empty() {
                        Arc::from("")
                    } else {
                        resolve(attr.prefix, &bindings, &frames, budget)?
                    };
                    let value = decode(attr.value, true, true, MAX_XML_BYTES, budget)?;
                    attrs.push(XmlValue::attribute(&namespace, attr.name, &value, budget)?);
                }
                let frame = Frame {
                    prefix: tag.prefix,
                    name: tag.name,
                    namespace,
                    bindings,
                    attributes: attrs,
                    children: Vec::new(),
                };
                if matches!(end, ElementEnd::Empty) {
                    let node = finish_frame(frame, budget)?;
                    append_node(node, &mut frames, &mut root, budget)?;
                } else {
                    frames.push(frame);
                }
            }
            Token::ElementEnd {
                end: ElementEnd::Close(prefix, name),
                ..
            } => {
                bound(prefix.as_str().len(), MAX_XML_NAME_BYTES, "name bytes")?;
                bound(name.as_str().len(), MAX_XML_NAME_BYTES, "name bytes")?;
                if pending.is_some() {
                    return Err(XmlError::Invalid);
                }
                let frame = frames.pop().ok_or(XmlError::Invalid)?;
                if !same(frame.prefix, prefix.as_str(), budget)? || !same(frame.name, name.as_str(), budget)? {
                    return Err(XmlError::Invalid);
                }
                let node = finish_frame(frame, budget)?;
                append_node(node, &mut frames, &mut root, budget)?;
            }
            Token::Text { text } | Token::Cdata { text, .. } => {
                if frames.is_empty() {
                    if matches!(token, Token::Cdata { .. })
                        || !text.as_str().chars().all(|c| matches!(c, ' ' | '\t' | '\r' | '\n'))
                    {
                        return Err(XmlError::Invalid);
                    }
                    continue;
                }
                nodes += 1;
                bound(nodes, MAX_XML_NODES, "nodes")?;
                let text = decode(
                    text.as_str(),
                    false,
                    matches!(token, Token::Text { .. }),
                    MAX_XML_BYTES,
                    budget,
                )?;
                let node = XmlValue::text(&text, budget)?;
                append_node(node, &mut frames, &mut root, budget)?;
            }
        }
    }
    if !frames.is_empty() || pending.is_some() {
        return Err(XmlError::Invalid);
    }
    root.ok_or(XmlError::Invalid)
}

#[cfg(test)]
#[path = "../tests/unit/xml_test.rs"]
mod tests;
