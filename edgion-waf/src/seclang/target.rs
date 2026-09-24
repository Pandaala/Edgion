use std::mem::size_of;
use std::ops::Range;

use thiserror::Error;

use super::{Directive, DirectiveDocument, LogicalLine, SourceDocument, SourcePosition, SourceSpan, Token};

pub(crate) const DEFAULT_MAX_TARGET_ATOMS_PER_EXPRESSION: usize = 1_024;
pub(crate) const DEFAULT_MAX_ID_SELECTORS_PER_UPDATE: usize = 1_024;
pub(crate) const DEFAULT_MAX_TARGET_NODES: usize = 200_000;
pub(crate) const DEFAULT_MAX_TARGET_OWNED_BYTES: usize = 32 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TargetSyntaxLimits {
    pub max_atoms_per_expression: usize,
    pub max_id_selectors_per_update: usize,
    pub max_nodes: usize,
    pub max_owned_bytes: usize,
}

impl Default for TargetSyntaxLimits {
    fn default() -> Self {
        Self {
            max_atoms_per_expression: DEFAULT_MAX_TARGET_ATOMS_PER_EXPRESSION,
            max_id_selectors_per_update: DEFAULT_MAX_ID_SELECTORS_PER_UPDATE,
            max_nodes: DEFAULT_MAX_TARGET_NODES,
            max_owned_bytes: DEFAULT_MAX_TARGET_OWNED_BYTES,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetModifier {
    None,
    Exclude,
    Count,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegexStyle {
    Bare,
    SingleQuoted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectorKind {
    None,
    Literal,
    Path,
    Regex(RegexStyle),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyntaxText<'source> {
    text: &'source str,
    logical_start: usize,
    logical_end: usize,
    span: SourceSpan,
}

impl<'source> SyntaxText<'source> {
    pub const fn text(&self) -> &'source str {
        self.text
    }

    pub fn logical_range(&self) -> Range<usize> {
        self.logical_start..self.logical_end
    }

    pub const fn span(&self) -> SourceSpan {
        self.span
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TargetAtom<'source> {
    modifier: TargetModifier,
    collection: SyntaxText<'source>,
    selector_kind: SelectorKind,
    selector: Option<SyntaxText<'source>>,
    regex_body: Option<SyntaxText<'source>>,
    span: SourceSpan,
}

impl<'source> TargetAtom<'source> {
    pub const fn modifier(&self) -> TargetModifier {
        self.modifier
    }

    pub const fn collection(&self) -> SyntaxText<'source> {
        self.collection
    }

    pub const fn selector_kind(&self) -> SelectorKind {
        self.selector_kind
    }

    pub const fn selector(&self) -> Option<SyntaxText<'source>> {
        self.selector
    }

    pub const fn regex_body(&self) -> Option<SyntaxText<'source>> {
        self.regex_body
    }

    pub const fn span(&self) -> SourceSpan {
        self.span
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdSelectorKind {
    Single(u64),
    Range { start: u64, end: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IdSelector<'source> {
    raw: &'source str,
    span: SourceSpan,
    kind: IdSelectorKind,
}

impl<'source> IdSelector<'source> {
    pub const fn raw(&self) -> &'source str {
        self.raw
    }

    pub const fn span(&self) -> SourceSpan {
        self.span
    }

    pub const fn kind(&self) -> IdSelectorKind {
        self.kind
    }
}

#[derive(Debug, PartialEq, Eq)]
enum TargetRecordKind {
    None,
    Rule {
        atom_start: usize,
        atom_count: usize,
    },
    Update {
        id_start: usize,
        id_count: usize,
        atom_start: usize,
        atom_count: usize,
    },
    Remove {
        id_start: usize,
        id_count: usize,
    },
}

#[derive(Debug, PartialEq, Eq)]
struct TargetRecord {
    logical_line_index: usize,
    kind: TargetRecordKind,
}

#[derive(Debug, Clone, Copy)]
pub enum DirectiveTargetView<'view, 'source> {
    None,
    Rule {
        atoms: &'view [TargetAtom<'source>],
    },
    Update {
        ids: &'view [IdSelector<'source>],
        atoms: &'view [TargetAtom<'source>],
    },
    Remove {
        ids: &'view [IdSelector<'source>],
    },
}

#[derive(Debug, Clone, Copy)]
pub struct TargetRecordView<'view, 'source> {
    logical_line_index: usize,
    targets: DirectiveTargetView<'view, 'source>,
}

impl<'view, 'source> TargetRecordView<'view, 'source> {
    pub const fn logical_line_index(&self) -> usize {
        self.logical_line_index
    }

    pub const fn targets(&self) -> DirectiveTargetView<'view, 'source> {
        self.targets
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct TargetSyntaxDocument<'source> {
    source: &'source SourceDocument,
    records: Vec<TargetRecord>,
    atoms: Vec<TargetAtom<'source>>,
    id_selectors: Vec<IdSelector<'source>>,
    charged_bytes: usize,
}

impl<'source> TargetSyntaxDocument<'source> {
    pub(crate) const fn source(&self) -> &'source SourceDocument {
        self.source
    }

    pub(crate) const fn charged_bytes(&self) -> usize {
        self.charged_bytes
    }

    pub(crate) fn atom_count(&self) -> usize {
        self.atoms.len()
    }

    pub(crate) fn id_selector_count(&self) -> usize {
        self.id_selectors.len()
    }

    pub(crate) fn record_count(&self) -> usize {
        self.records.len()
    }

    pub(super) fn get(&self, index: usize) -> Option<TargetRecordView<'_, 'source>> {
        let record = self.records.get(index)?;
        let targets = match record.kind {
            TargetRecordKind::None => DirectiveTargetView::None,
            TargetRecordKind::Rule { atom_start, atom_count } => DirectiveTargetView::Rule {
                atoms: checked_slice(&self.atoms, atom_start, atom_count, "rule atom range must remain valid"),
            },
            TargetRecordKind::Update {
                id_start,
                id_count,
                atom_start,
                atom_count,
            } => DirectiveTargetView::Update {
                ids: checked_slice(
                    &self.id_selectors,
                    id_start,
                    id_count,
                    "update ID range must remain valid",
                ),
                atoms: checked_slice(
                    &self.atoms,
                    atom_start,
                    atom_count,
                    "update atom range must remain valid",
                ),
            },
            TargetRecordKind::Remove { id_start, id_count } => DirectiveTargetView::Remove {
                ids: checked_slice(
                    &self.id_selectors,
                    id_start,
                    id_count,
                    "remove ID range must remain valid",
                ),
            },
        };
        Some(TargetRecordView {
            logical_line_index: record.logical_line_index,
            targets,
        })
    }
}

fn checked_slice<'a, T>(values: &'a [T], start: usize, count: usize, reason: &'static str) -> &'a [T] {
    let end = start.checked_add(count).expect(reason);
    values.get(start..end).expect(reason)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModifierErrorKind {
    Repeated,
    Mixed,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum TargetSyntaxError {
    #[error("target expression exceeds its atom limit of {limit} at {position:?}")]
    AtomLimit { limit: usize, position: SourcePosition },
    #[error("rule ID directive exceeds its ID-selector limit of {limit} at {position:?}")]
    IdSelectorLimit { limit: usize, position: SourcePosition },
    #[error("target syntax exceeds its combined node limit of {limit} at {position:?}")]
    NodeLimit { limit: usize, position: SourcePosition },
    #[error("target expression contains an empty atom at {position:?}")]
    EmptyAtom { position: SourcePosition },
    #[error("target atom contains an empty collection at {position:?}")]
    EmptyCollection { position: SourcePosition },
    #[error("target atom has a {kind:?} leading modifier at {position:?}")]
    InvalidModifier {
        kind: ModifierErrorKind,
        position: SourcePosition,
    },
    #[error("target selector has an unclosed {style:?} regex opened at {position:?}")]
    UnclosedRegex {
        style: RegexStyle,
        position: SourcePosition,
    },
    #[error("single-quoted target regex has no closing quote at {position:?}")]
    UnclosedRegexQuote { position: SourcePosition },
    #[error("target regex has a suffix after its closure at {position:?}")]
    RegexSuffix { position: SourcePosition },
    #[error("single-target syntax contains a forbidden modifier at {position:?}")]
    SingleTargetModifier { position: SourcePosition },
    #[error("single-target syntax contains a forbidden expression delimiter at {position:?}")]
    SingleTargetDelimiter { position: SourcePosition },
    #[error("target update ID selector has invalid decimal syntax at {position:?}")]
    InvalidIdSelector { position: SourcePosition },
    #[error("target update ID selector exceeds u64 at {position:?}")]
    IdSelectorOverflow { position: SourcePosition },
    #[error("cannot map target syntax on logical line {logical_line} at byte offset {logical_offset}")]
    PositionMapping { logical_line: usize, logical_offset: usize },
    #[error("target syntax requires more than {limit} charged owned bytes at {position:?}")]
    OwnedByteLimit { limit: usize, position: SourcePosition },
    #[error("cannot allocate bounded SecLang {operation}; reduce the source or configured limits")]
    Allocation { operation: &'static str },
}

pub(super) fn parse_single_target<'source>(
    text: &'source str,
    logical_start: usize,
    line: &'source LogicalLine,
    line_index: usize,
) -> Result<TargetAtom<'source>, TargetSyntaxError> {
    if matches!(text.as_bytes().first(), Some(b'!' | b'&')) {
        return Err(TargetSyntaxError::SingleTargetModifier {
            position: map_start(line, line_index, logical_start)?,
        });
    }
    let located = locate_atom(text, 0, line, line_index, logical_start)?;
    if located.end != text.len() {
        return Err(TargetSyntaxError::SingleTargetDelimiter {
            position: map_start(line, line_index, add_offset(logical_start, located.end, line_index)?)?,
        });
    }
    parse_atom(text, 0, located, line, line_index, logical_start)
}

pub(super) fn parse_target_syntax<'source>(
    directives: &DirectiveDocument<'source>,
    limits: TargetSyntaxLimits,
) -> Result<TargetSyntaxDocument<'source>, TargetSyntaxError> {
    let source = directives.source();
    let mut document = TargetSyntaxDocument {
        source,
        records: Vec::new(),
        atoms: Vec::new(),
        id_selectors: Vec::new(),
        charged_bytes: 0,
    };

    for directive in directives.iter() {
        let line_index = directive.logical_line_index();
        let line = source
            .lines()
            .get(line_index)
            .ok_or(TargetSyntaxError::PositionMapping {
                logical_line: line_index + 1,
                logical_offset: 0,
            })?;
        let kind = match directive.directive() {
            Directive::SecRule { targets, .. } => {
                let atom_start = document.atoms.len();
                parse_expression(targets, line, line_index, limits, &mut document)?;
                TargetRecordKind::Rule {
                    atom_start,
                    atom_count: document.atoms.len() - atom_start,
                }
            }
            Directive::SecRuleUpdateTargetById { selectors, targets } => {
                let id_start = document.id_selectors.len();
                parse_id_selectors(selectors, line, line_index, limits, &mut document)?;
                let atom_start = document.atoms.len();
                parse_expression(targets, line, line_index, limits, &mut document)?;
                TargetRecordKind::Update {
                    id_start,
                    id_count: document.id_selectors.len() - id_start,
                    atom_start,
                    atom_count: document.atoms.len() - atom_start,
                }
            }
            Directive::SecRuleRemoveById { selectors } => {
                let id_start = document.id_selectors.len();
                parse_id_selectors(selectors, line, line_index, limits, &mut document)?;
                TargetRecordKind::Remove {
                    id_start,
                    id_count: document.id_selectors.len() - id_start,
                }
            }
            _ => TargetRecordKind::None,
        };

        charge_owned(
            &mut document.charged_bytes,
            size_of::<TargetRecord>(),
            limits.max_owned_bytes,
            directive.span().start,
        )?;
        document
            .records
            .try_reserve(1)
            .map_err(|_| TargetSyntaxError::Allocation {
                operation: "per-directive target records",
            })?;
        document.records.push(TargetRecord {
            logical_line_index: line_index,
            kind,
        });
    }
    Ok(document)
}

fn parse_id_selectors<'source>(
    selectors: &[Token<'source>],
    line: &'source LogicalLine,
    line_index: usize,
    limits: TargetSyntaxLimits,
    document: &mut TargetSyntaxDocument<'source>,
) -> Result<(), TargetSyntaxError> {
    for (selector_index, selector) in selectors.iter().copied().enumerate() {
        if selector_index >= limits.max_id_selectors_per_update {
            return Err(TargetSyntaxError::IdSelectorLimit {
                limit: limits.max_id_selectors_per_update,
                position: selector.content_span().start,
            });
        }
        ensure_node_limit(document, limits.max_nodes, selector.content_span().start)?;
        let parsed = parse_id_selector(selector, line, line_index)?;
        charge_owned(
            &mut document.charged_bytes,
            size_of::<IdSelector<'source>>(),
            limits.max_owned_bytes,
            selector.content_span().start,
        )?;
        document
            .id_selectors
            .try_reserve(1)
            .map_err(|_| TargetSyntaxError::Allocation {
                operation: "rule ID selector records",
            })?;
        document.id_selectors.push(parsed);
    }
    Ok(())
}

fn parse_expression<'source>(
    token: Token<'source>,
    line: &'source LogicalLine,
    line_index: usize,
    limits: TargetSyntaxLimits,
    document: &mut TargetSyntaxDocument<'source>,
) -> Result<(), TargetSyntaxError> {
    let text = token.value();
    let base = token.content_logical_range().start;
    let mut local_start = 0;
    let mut expression_count = 0;
    loop {
        let position = map_start(line, line_index, add_offset(base, local_start, line_index)?)?;
        if expression_count >= limits.max_atoms_per_expression {
            return Err(TargetSyntaxError::AtomLimit {
                limit: limits.max_atoms_per_expression,
                position,
            });
        }
        ensure_node_limit(document, limits.max_nodes, position)?;

        let located = locate_atom(text, local_start, line, line_index, base)?;
        let atom = parse_atom(text, local_start, located, line, line_index, base)?;
        charge_owned(
            &mut document.charged_bytes,
            size_of::<TargetAtom<'source>>(),
            limits.max_owned_bytes,
            position,
        )?;
        document
            .atoms
            .try_reserve(1)
            .map_err(|_| TargetSyntaxError::Allocation {
                operation: "target atom records",
            })?;
        document.atoms.push(atom);
        expression_count += 1;

        if located.end == text.len() {
            break;
        }
        local_start = located.end + 1;
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct LocatedAtom {
    end: usize,
    colon: Option<usize>,
    regex: Option<LocatedRegex>,
}

#[derive(Clone, Copy)]
struct LocatedRegex {
    style: RegexStyle,
    selector_start: usize,
    body_start: usize,
    body_end: usize,
    selector_end: usize,
}

fn locate_atom(
    text: &str,
    start: usize,
    line: &LogicalLine,
    line_index: usize,
    base: usize,
) -> Result<LocatedAtom, TargetSyntaxError> {
    let bytes = text.as_bytes();
    if start == bytes.len() {
        return Ok(LocatedAtom {
            end: start,
            colon: None,
            regex: None,
        });
    }
    let ordinary_end = bytes[start..]
        .iter()
        .position(|byte| *byte == b'|')
        .map_or(bytes.len(), |relative| start + relative);
    let colon = bytes[start..ordinary_end]
        .iter()
        .position(|byte| *byte == b':')
        .map(|relative| start + relative);
    let Some(colon_offset) = colon else {
        return Ok(LocatedAtom {
            end: ordinary_end,
            colon: None,
            regex: None,
        });
    };

    let mut collection_start = start;
    while collection_start < colon_offset && matches!(bytes[collection_start], b'!' | b'&') {
        collection_start += 1;
    }
    let collection = &text[collection_start..colon_offset];
    if collection.eq_ignore_ascii_case("XML") || collection.eq_ignore_ascii_case("JSON") {
        return Ok(LocatedAtom {
            end: ordinary_end,
            colon,
            regex: None,
        });
    }

    let selector_start = colon_offset + 1;
    let (style, slash_start, body_start) = if bytes.get(selector_start) == Some(&b'/') {
        (RegexStyle::Bare, selector_start, selector_start + 1)
    } else if bytes.get(selector_start..selector_start + 2) == Some(&[b'\'', b'/'][..]) {
        (RegexStyle::SingleQuoted, selector_start + 1, selector_start + 2)
    } else {
        return Ok(LocatedAtom {
            end: ordinary_end,
            colon,
            regex: None,
        });
    };

    let closing_slash = find_regex_close(bytes, body_start).ok_or(TargetSyntaxError::UnclosedRegex {
        style,
        position: map_start(line, line_index, add_offset(base, slash_start, line_index)?)?,
    })?;
    let (selector_end, end) = match style {
        RegexStyle::Bare => {
            let after = closing_slash + 1;
            if after < bytes.len() && bytes[after] != b'|' {
                return Err(TargetSyntaxError::RegexSuffix {
                    position: map_start(line, line_index, add_offset(base, after, line_index)?)?,
                });
            }
            (after, after)
        }
        RegexStyle::SingleQuoted => {
            let quote = closing_slash + 1;
            if bytes.get(quote) != Some(&b'\'') {
                return Err(TargetSyntaxError::UnclosedRegexQuote {
                    position: map_start(line, line_index, add_offset(base, selector_start, line_index)?)?,
                });
            }
            let after = quote + 1;
            if after < bytes.len() && bytes[after] != b'|' {
                return Err(TargetSyntaxError::RegexSuffix {
                    position: map_start(line, line_index, add_offset(base, after, line_index)?)?,
                });
            }
            (after, after)
        }
    };
    Ok(LocatedAtom {
        end,
        colon,
        regex: Some(LocatedRegex {
            style,
            selector_start,
            body_start,
            body_end: closing_slash,
            selector_end,
        }),
    })
}

fn find_regex_close(bytes: &[u8], start: usize) -> Option<usize> {
    let mut backslash_run = 0_usize;
    for (offset, byte) in bytes.iter().copied().enumerate().skip(start) {
        if byte == b'/' && backslash_run.is_multiple_of(2) {
            return Some(offset);
        }
        if byte == b'\\' {
            backslash_run = backslash_run.saturating_add(1);
        } else {
            backslash_run = 0;
        }
    }
    None
}

fn parse_atom<'source>(
    text: &'source str,
    start: usize,
    located: LocatedAtom,
    line: &LogicalLine,
    line_index: usize,
    base: usize,
) -> Result<TargetAtom<'source>, TargetSyntaxError> {
    if start == located.end {
        return Err(TargetSyntaxError::EmptyAtom {
            position: map_start(line, line_index, add_offset(base, start, line_index)?)?,
        });
    }
    let bytes = text.as_bytes();
    let (modifier, collection_start) = match bytes[start] {
        b'!' => (TargetModifier::Exclude, start + 1),
        b'&' => (TargetModifier::Count, start + 1),
        _ => (TargetModifier::None, start),
    };
    if collection_start < located.end && matches!(bytes[collection_start], b'!' | b'&') {
        let kind = if bytes[start] == bytes[collection_start] {
            ModifierErrorKind::Repeated
        } else {
            ModifierErrorKind::Mixed
        };
        return Err(TargetSyntaxError::InvalidModifier {
            kind,
            position: map_start(line, line_index, add_offset(base, collection_start, line_index)?)?,
        });
    }
    let collection_end = located.colon.unwrap_or(located.end);
    if collection_start == collection_end {
        return Err(TargetSyntaxError::EmptyCollection {
            position: map_start(line, line_index, add_offset(base, collection_start, line_index)?)?,
        });
    }
    let collection = syntax_text(text, collection_start, collection_end, line, line_index, base)?;
    let (selector_kind, selector, regex_body) = match (located.colon, located.regex) {
        (None, None) => (SelectorKind::None, None, None),
        (Some(colon), None) => {
            let selector_start = colon + 1;
            let selector = syntax_text(text, selector_start, located.end, line, line_index, base)?;
            let kind = if collection.text.eq_ignore_ascii_case("XML") || collection.text.eq_ignore_ascii_case("JSON") {
                SelectorKind::Path
            } else {
                SelectorKind::Literal
            };
            (kind, Some(selector), None)
        }
        (Some(_), Some(regex)) => (
            SelectorKind::Regex(regex.style),
            Some(syntax_text(
                text,
                regex.selector_start,
                regex.selector_end,
                line,
                line_index,
                base,
            )?),
            Some(syntax_text(
                text,
                regex.body_start,
                regex.body_end,
                line,
                line_index,
                base,
            )?),
        ),
        (None, Some(_)) => {
            return Err(TargetSyntaxError::PositionMapping {
                logical_line: line_index + 1,
                logical_offset: add_offset(base, start, line_index)?,
            });
        }
    };
    Ok(TargetAtom {
        modifier,
        collection,
        selector_kind,
        selector,
        regex_body,
        span: map_span(
            line,
            line_index,
            add_offset(base, start, line_index)?,
            add_offset(base, located.end, line_index)?,
        )?,
    })
}

fn syntax_text<'source>(
    text: &'source str,
    start: usize,
    end: usize,
    line: &LogicalLine,
    line_index: usize,
    base: usize,
) -> Result<SyntaxText<'source>, TargetSyntaxError> {
    let logical_start = add_offset(base, start, line_index)?;
    let logical_end = add_offset(base, end, line_index)?;
    Ok(SyntaxText {
        text: text.get(start..end).ok_or(TargetSyntaxError::PositionMapping {
            logical_line: line_index + 1,
            logical_offset: logical_start,
        })?,
        logical_start,
        logical_end,
        span: map_span(line, line_index, logical_start, logical_end)?,
    })
}

fn parse_id_selector<'source>(
    token: Token<'source>,
    line: &LogicalLine,
    line_index: usize,
) -> Result<IdSelector<'source>, TargetSyntaxError> {
    let text = token.value();
    let base = token.content_logical_range().start;
    let mut hyphen = None;
    for (offset, byte) in text.bytes().enumerate() {
        if byte == b'-' {
            if hyphen.replace(offset).is_some() {
                return Err(TargetSyntaxError::InvalidIdSelector {
                    position: map_start(line, line_index, add_offset(base, offset, line_index)?)?,
                });
            }
        } else if !byte.is_ascii_digit() {
            return Err(TargetSyntaxError::InvalidIdSelector {
                position: map_start(line, line_index, add_offset(base, offset, line_index)?)?,
            });
        }
    }
    let kind = if let Some(separator) = hyphen {
        if separator == 0 || separator + 1 == text.len() {
            return Err(TargetSyntaxError::InvalidIdSelector {
                position: map_start(line, line_index, add_offset(base, separator, line_index)?)?,
            });
        }
        IdSelectorKind::Range {
            start: parse_u64(&text[..separator], base, line, line_index)?,
            end: parse_u64(
                &text[separator + 1..],
                add_offset(base, separator + 1, line_index)?,
                line,
                line_index,
            )?,
        }
    } else {
        if text.is_empty() {
            return Err(TargetSyntaxError::InvalidIdSelector {
                position: map_start(line, line_index, base)?,
            });
        }
        IdSelectorKind::Single(parse_u64(text, base, line, line_index)?)
    };
    Ok(IdSelector {
        raw: text,
        span: token.content_span(),
        kind,
    })
}

fn parse_u64(digits: &str, base: usize, line: &LogicalLine, line_index: usize) -> Result<u64, TargetSyntaxError> {
    let mut value = 0_u64;
    for (offset, byte) in digits.bytes().enumerate() {
        value = value
            .checked_mul(10)
            .and_then(|current| current.checked_add(u64::from(byte - b'0')))
            .ok_or(TargetSyntaxError::IdSelectorOverflow {
                position: map_start(line, line_index, add_offset(base, offset, line_index)?)?,
            })?;
    }
    Ok(value)
}

fn ensure_node_limit(
    document: &TargetSyntaxDocument<'_>,
    limit: usize,
    position: SourcePosition,
) -> Result<(), TargetSyntaxError> {
    let count = document
        .atoms
        .len()
        .checked_add(document.id_selectors.len())
        .ok_or(TargetSyntaxError::NodeLimit { limit, position })?;
    if count >= limit {
        Err(TargetSyntaxError::NodeLimit { limit, position })
    } else {
        Ok(())
    }
}

fn add_offset(base: usize, relative: usize, line_index: usize) -> Result<usize, TargetSyntaxError> {
    base.checked_add(relative).ok_or(TargetSyntaxError::PositionMapping {
        logical_line: line_index + 1,
        logical_offset: base,
    })
}

fn map_start(
    line: &LogicalLine,
    line_index: usize,
    logical_offset: usize,
) -> Result<SourcePosition, TargetSyntaxError> {
    line.position_at(logical_offset)
        .ok_or(TargetSyntaxError::PositionMapping {
            logical_line: line_index + 1,
            logical_offset,
        })
}

fn map_span(line: &LogicalLine, line_index: usize, start: usize, end: usize) -> Result<SourceSpan, TargetSyntaxError> {
    let start_position = map_start(line, line_index, start)?;
    let end_position = if start == end {
        start_position
    } else {
        line.position_at_end(end).ok_or(TargetSyntaxError::PositionMapping {
            logical_line: line_index + 1,
            logical_offset: end,
        })?
    };
    Ok(SourceSpan {
        start: start_position,
        end: end_position,
    })
}

fn charge_owned(
    charged: &mut usize,
    amount: usize,
    limit: usize,
    position: SourcePosition,
) -> Result<(), TargetSyntaxError> {
    let next = charged
        .checked_add(amount)
        .ok_or(TargetSyntaxError::OwnedByteLimit { limit, position })?;
    if next > limit {
        return Err(TargetSyntaxError::OwnedByteLimit { limit, position });
    }
    *charged = next;
    Ok(())
}

#[cfg(test)]
#[path = "../../tests/unit/seclang/target_test.rs"]
mod tests;
