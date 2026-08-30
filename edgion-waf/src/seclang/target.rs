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

    #[cfg(test)]
    pub(crate) fn remove_last_record_for_test(&mut self) {
        self.records.pop();
    }

    #[cfg(test)]
    pub(super) fn swap_records_for_test(&mut self, left: usize, right: usize) {
        self.records.swap(left, right);
    }

    #[cfg(test)]
    pub(super) fn set_first_record_none_for_test(&mut self) {
        if let Some(record) = self.records.first_mut() {
            record.kind = TargetRecordKind::None;
        }
    }

    #[cfg(test)]
    pub(crate) fn iter(&self) -> impl Iterator<Item = TargetRecordView<'_, 'source>> {
        self.records.iter().map(|record| {
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
            TargetRecordView {
                logical_line_index: record.logical_line_index,
                targets,
            }
        })
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
mod tests {
    use std::fs;
    use std::mem::size_of;
    use std::path::{Path, PathBuf};

    use super::{
        parse_target_syntax, DirectiveTargetView, IdSelectorKind, ModifierErrorKind, RegexStyle, SelectorKind,
        TargetAtom, TargetModifier, TargetRecord, TargetSyntaxError, TargetSyntaxLimits,
    };
    use crate::seclang::{
        parse_directives, scan_source, tokenize_document, DirectiveLimits, SourceLimits, TokenizerLimits,
    };

    fn with_syntax(source: &[u8], inspect: impl FnOnce(&super::TargetSyntaxDocument<'_>)) {
        let source = scan_source("test", source, SourceLimits::default()).unwrap();
        let tokenized = tokenize_document(&source, TokenizerLimits::default()).unwrap();
        let directives = parse_directives(&tokenized, DirectiveLimits::default()).unwrap();
        let syntax = parse_target_syntax(&directives, TargetSyntaxLimits::default()).unwrap();
        inspect(&syntax);
    }

    fn syntax_error(source: &[u8], limits: TargetSyntaxLimits) -> TargetSyntaxError {
        let source = scan_source("test", source, SourceLimits::default()).unwrap();
        let tokenized = tokenize_document(&source, TokenizerLimits::default()).unwrap();
        let directives = parse_directives(&tokenized, DirectiveLimits::default()).unwrap();
        parse_target_syntax(&directives, limits).unwrap_err()
    }

    #[test]
    fn parses_modifiers_literal_empty_path_and_regex_selectors() {
        with_syntax(
            br##"SecRule ARGS|!REQUEST_HEADERS:name|&FILES|COL:|XML:/a/b|json:/x|COL:/a|b\/c/|COL:'/d\/e/' """##,
            |syntax| {
                let record = syntax.iter().next().unwrap();
                let DirectiveTargetView::Rule { atoms } = record.targets() else {
                    panic!("expected rule targets");
                };
                assert_eq!(atoms.len(), 8);
                assert_eq!(atoms[0].modifier(), TargetModifier::None);
                assert_eq!(atoms[0].selector_kind(), SelectorKind::None);
                assert_eq!(atoms[1].modifier(), TargetModifier::Exclude);
                assert_eq!(atoms[1].selector().unwrap().text(), "name");
                assert_eq!(atoms[2].modifier(), TargetModifier::Count);
                assert_eq!(atoms[3].selector_kind(), SelectorKind::Literal);
                assert_eq!(atoms[3].selector().unwrap().text(), "");
                assert_eq!(atoms[4].selector_kind(), SelectorKind::Path);
                assert_eq!(atoms[4].selector().unwrap().text(), "/a/b");
                assert_eq!(atoms[5].selector_kind(), SelectorKind::Path);
                assert_eq!(atoms[6].selector_kind(), SelectorKind::Regex(RegexStyle::Bare));
                assert_eq!(atoms[6].regex_body().unwrap().text(), "a|b\\/c");
                assert_eq!(atoms[7].selector_kind(), SelectorKind::Regex(RegexStyle::SingleQuoted));
                assert_eq!(atoms[7].regex_body().unwrap().text(), "d\\/e");
            },
        );
    }

    #[test]
    fn regex_closure_uses_backslash_run_parity_and_accepts_empty_body() {
        with_syntax(b"SecRule A://|B:/x\\\\/|C:/x\\/y/ \"\"", |syntax| {
            let DirectiveTargetView::Rule { atoms } = syntax.iter().next().unwrap().targets() else {
                panic!("expected rule targets");
            };
            assert_eq!(atoms[0].regex_body().unwrap().text(), "");
            assert_eq!(atoms[1].regex_body().unwrap().text(), "x\\\\");
            assert_eq!(atoms[2].regex_body().unwrap().text(), "x\\/y");
        });
    }

    #[test]
    fn rejects_empty_atoms_and_modifier_shapes() {
        for source in [&b"SecRule |ARGS \"\""[..], &b"SecRule ARGS| \"\""[..]] {
            assert!(matches!(
                syntax_error(source, TargetSyntaxLimits::default()),
                TargetSyntaxError::EmptyAtom { .. }
            ));
        }
        for (source, kind) in [
            (&b"SecRule !!ARGS \"\""[..], ModifierErrorKind::Repeated),
            (&b"SecRule !&ARGS \"\""[..], ModifierErrorKind::Mixed),
            (&b"SecRule &&ARGS \"\""[..], ModifierErrorKind::Repeated),
        ] {
            assert!(matches!(
                syntax_error(source, TargetSyntaxLimits::default()),
                TargetSyntaxError::InvalidModifier { kind: actual, .. } if actual == kind
            ));
        }
        assert!(matches!(
            syntax_error(b"SecRule ! \"\"", TargetSyntaxLimits::default()),
            TargetSyntaxError::EmptyCollection { .. }
        ));
    }

    #[test]
    fn rejects_unclosed_regex_quote_and_closure_suffix_without_content() {
        for (source, expected) in [
            (&b"SecRule A:/private \"\""[..], "regex"),
            (&b"SecRule A:'/private/ \"\""[..], "quote"),
            (&b"SecRule A:/private/x \"\""[..], "suffix"),
        ] {
            let error = syntax_error(source, TargetSyntaxLimits::default());
            match expected {
                "regex" => assert!(matches!(error, TargetSyntaxError::UnclosedRegex { .. })),
                "quote" => assert!(matches!(error, TargetSyntaxError::UnclosedRegexQuote { .. })),
                _ => assert!(matches!(error, TargetSyntaxError::RegexSuffix { .. })),
            }
            assert!(!error.to_string().contains("private"));
            assert!(!format!("{error:?}").contains("private"));
        }
        assert!(matches!(
            syntax_error(
                br##"SecRuleUpdateTargetById 1 "A:/private\\""##,
                TargetSyntaxLimits::default()
            ),
            TargetSyntaxError::UnclosedRegex { .. }
        ));
    }

    #[test]
    fn parses_update_ids_without_expanding_ranges_and_uses_quoted_content_ranges() {
        with_syntax(
            b"SecRuleUpdateTargetById 0 18446744073709551615 9-3 \"ARGS|XML:/x\"",
            |syntax| {
                assert_eq!(syntax.id_selector_count(), 3);
                let record = syntax.iter().next().unwrap();
                let DirectiveTargetView::Update { ids, atoms } = record.targets() else {
                    panic!("expected update targets");
                };
                assert_eq!(ids[0].kind(), IdSelectorKind::Single(0));
                assert_eq!(ids[1].kind(), IdSelectorKind::Single(u64::MAX));
                assert_eq!(ids[2].kind(), IdSelectorKind::Range { start: 9, end: 3 });
                assert_eq!(ids[2].raw(), "9-3");
                assert_eq!(atoms.len(), 2);
                assert_eq!(atoms[0].collection().text(), "ARGS");
                assert_eq!(atoms[0].span().start.column, 53);
            },
        );
    }

    #[test]
    fn parses_remove_ids_in_the_shared_flat_arena_without_targets() {
        let source = scan_source(
            "remove",
            b"SecRuleRemoveById 0 18446744073709551615 9-\\\n3",
            SourceLimits::default(),
        )
        .unwrap();
        let tokenized = tokenize_document(&source, TokenizerLimits::default()).unwrap();
        let directives = parse_directives(&tokenized, DirectiveLimits::default()).unwrap();
        let exact_bytes = size_of::<TargetRecord>() + 3 * size_of::<super::IdSelector<'_>>();
        let syntax = parse_target_syntax(
            &directives,
            TargetSyntaxLimits {
                max_owned_bytes: exact_bytes,
                ..TargetSyntaxLimits::default()
            },
        )
        .unwrap();
        assert_eq!(syntax.charged_bytes(), exact_bytes);
        assert_eq!(syntax.atom_count(), 0);
        let DirectiveTargetView::Remove { ids } = syntax.iter().next().unwrap().targets() else {
            panic!("expected remove selector mapping")
        };
        assert_eq!(
            ids.iter().map(|id| id.kind()).collect::<Vec<_>>(),
            [
                IdSelectorKind::Single(0),
                IdSelectorKind::Single(u64::MAX),
                IdSelectorKind::Range { start: 9, end: 3 },
            ]
        );
        assert_eq!(ids[2].raw(), "9-3");
        assert_eq!((ids[2].span().start.line, ids[2].span().end.line), (1, 2));
        assert!(matches!(
            parse_target_syntax(
                &directives,
                TargetSyntaxLimits {
                    max_owned_bytes: exact_bytes - 1,
                    ..TargetSyntaxLimits::default()
                }
            ),
            Err(TargetSyntaxError::OwnedByteLimit { .. })
        ));
    }

    #[test]
    fn rejects_every_malformed_id_shape_and_overflow() {
        for selector in ["+1", "-1", "1-", "1-2-3", "1a", "--"] {
            for source in [
                format!("SecRuleUpdateTargetById {selector} ARGS"),
                format!("SecRuleRemoveById {selector}"),
            ] {
                assert!(matches!(
                    syntax_error(source.as_bytes(), TargetSyntaxLimits::default()),
                    TargetSyntaxError::InvalidIdSelector { .. }
                ));
            }
        }
        assert!(matches!(
            syntax_error(
                b"SecRuleUpdateTargetById 18446744073709551616 ARGS",
                TargetSyntaxLimits::default()
            ),
            TargetSyntaxError::IdSelectorOverflow { .. }
        ));
        assert!(matches!(
            syntax_error(b"SecRuleRemoveById 18446744073709551616", TargetSyntaxLimits::default()),
            TargetSyntaxError::IdSelectorOverflow { .. }
        ));
    }

    #[test]
    fn per_expression_and_per_update_limits_precede_total_limit() {
        let atom_limits = TargetSyntaxLimits {
            max_atoms_per_expression: 1,
            max_nodes: 1,
            ..TargetSyntaxLimits::default()
        };
        assert!(matches!(
            syntax_error(b"SecRule ARGS| \"\"", atom_limits),
            TargetSyntaxError::AtomLimit { limit: 1, .. }
        ));

        let id_limits = TargetSyntaxLimits {
            max_id_selectors_per_update: 1,
            max_nodes: 1,
            ..TargetSyntaxLimits::default()
        };
        assert!(matches!(
            syntax_error(b"SecRuleUpdateTargetById 1 + ARGS", id_limits),
            TargetSyntaxError::IdSelectorLimit { limit: 1, .. }
        ));
        assert!(matches!(
            syntax_error(b"SecRuleRemoveById 1 +", id_limits),
            TargetSyntaxError::IdSelectorLimit { limit: 1, .. }
        ));

        let node_limits = TargetSyntaxLimits {
            max_nodes: 0,
            ..TargetSyntaxLimits::default()
        };
        assert!(matches!(
            syntax_error(b"SecRule |ARGS \"\"", node_limits),
            TargetSyntaxError::NodeLimit { limit: 0, .. }
        ));
        assert!(matches!(
            syntax_error(b"SecRuleUpdateTargetById + ARGS", node_limits),
            TargetSyntaxError::NodeLimit { limit: 0, .. }
        ));
        assert!(matches!(
            syntax_error(b"SecRuleRemoveById +", node_limits),
            TargetSyntaxError::NodeLimit { limit: 0, .. }
        ));
    }

    #[test]
    fn enforces_exact_owned_metadata_and_preserves_safe_flat_views() {
        let source = scan_source(
            "metadata",
            b"SecAction \"id:1\"\nSecRuleUpdateTargetById 1 ARGS|!ARGS:a",
            SourceLimits::default(),
        )
        .unwrap();
        let tokenized = tokenize_document(&source, TokenizerLimits::default()).unwrap();
        let directives = parse_directives(&tokenized, DirectiveLimits::default()).unwrap();
        let exact_bytes =
            2 * size_of::<TargetRecord>() + 2 * size_of::<TargetAtom<'_>>() + size_of::<super::IdSelector<'_>>();
        let exact = TargetSyntaxLimits {
            max_owned_bytes: exact_bytes,
            ..TargetSyntaxLimits::default()
        };
        let syntax = parse_target_syntax(&directives, exact).unwrap();
        assert_eq!(syntax.charged_bytes(), exact_bytes);
        assert_eq!(syntax.source().source_id(), "metadata");
        assert_eq!(syntax.iter().count(), 2);
        assert_eq!(syntax.iter().nth(1).unwrap().logical_line_index(), 1);
        let DirectiveTargetView::Update { atoms, .. } = syntax.iter().nth(1).unwrap().targets() else {
            panic!("expected update targets");
        };
        assert_eq!(atoms[1].collection().logical_range(), 32..36);
        assert_eq!(atoms[1].collection().span(), atoms[1].collection().span());
        let first_range = atoms[0].collection().logical_range();
        assert!(std::ptr::eq(
            atoms[0].collection().text().as_ptr(),
            directives.source().lines()[1].text()[first_range].as_ptr()
        ));

        let short = TargetSyntaxLimits {
            max_owned_bytes: exact_bytes - 1,
            ..TargetSyntaxLimits::default()
        };
        assert!(matches!(
            parse_target_syntax(&directives, short),
            Err(TargetSyntaxError::OwnedByteLimit { .. })
        ));
    }

    #[test]
    fn maps_utf8_and_continuation_join_positions() {
        let error = syntax_error("SecRule café| \"\"".as_bytes(), TargetSyntaxLimits::default());
        assert!(matches!(
            error,
            TargetSyntaxError::EmptyAtom {
                position: crate::seclang::SourcePosition { column: 15, .. }
            }
        ));

        with_syntax(b"SecRuleUpdateTargetById 1 \"ARGS|\\\n XML:/x\"", |syntax| {
            let DirectiveTargetView::Update { atoms, .. } = syntax.iter().next().unwrap().targets() else {
                panic!("expected update targets");
            };
            assert_eq!(atoms[1].span().start.line, 2);
            assert_eq!(atoms[0].span().end.line, 1);
        });
    }

    #[test]
    fn whole_document_failure_publishes_no_partial_arena() {
        let error = syntax_error(
            b"SecRule ARGS \"\"\nSecRule PRIVATE:/unterminated \"\"",
            TargetSyntaxLimits::default(),
        );
        assert!(matches!(error, TargetSyntaxError::UnclosedRegex { .. }));
    }

    #[test]
    fn pinned_crs_target_corpus_gate_when_configured() {
        let Ok(root) = std::env::var("EDGION_WAF_CRS_RULES_DIR") else {
            return;
        };
        let mut files = Vec::new();
        collect_conf_files(Path::new(&root), &mut files);
        files.sort();
        let file_count = files.len();
        let mut rule_atoms = 0_usize;
        let mut update_atoms = 0_usize;
        let mut update_ids = 0_usize;
        let mut max_atoms_per_expression = 0_usize;
        let mut modifiers = [0_usize; 3];
        let mut selector_kinds = [0_usize; 4];
        let mut directive_count = 0_usize;
        let mut directive_charged = 0_usize;
        let mut target_charged = 0_usize;
        for file in files {
            let bytes = fs::read(&file).expect("pinned CRS file must remain readable during the corpus test");
            let source_id = file.to_string_lossy();
            let source = scan_source(&source_id, &bytes, SourceLimits::default()).unwrap();
            let tokenized = tokenize_document(&source, TokenizerLimits::default()).unwrap();
            let directives = parse_directives(&tokenized, DirectiveLimits::default()).unwrap();
            let syntax = parse_target_syntax(&directives, TargetSyntaxLimits::default()).unwrap();
            directive_count += directives.len();
            directive_charged += directives.charged_bytes();
            target_charged += syntax.charged_bytes();
            for record in syntax.iter() {
                match record.targets() {
                    DirectiveTargetView::None => {}
                    DirectiveTargetView::Rule { atoms } => {
                        max_atoms_per_expression = max_atoms_per_expression.max(atoms.len());
                        rule_atoms += atoms.len();
                        for atom in atoms {
                            modifiers[match atom.modifier() {
                                TargetModifier::None => 0,
                                TargetModifier::Exclude => 1,
                                TargetModifier::Count => 2,
                            }] += 1;
                            selector_kinds[match atom.selector_kind() {
                                SelectorKind::None => 0,
                                SelectorKind::Literal => 1,
                                SelectorKind::Path => 2,
                                SelectorKind::Regex(_) => 3,
                            }] += 1;
                        }
                    }
                    DirectiveTargetView::Update { ids, atoms } => {
                        max_atoms_per_expression = max_atoms_per_expression.max(atoms.len());
                        update_ids += ids.len();
                        update_atoms += atoms.len();
                    }
                    DirectiveTargetView::Remove { ids } => update_ids += ids.len(),
                }
            }
        }
        assert_eq!(file_count, 27);
        assert_eq!(directive_count, 786);
        eprintln!(
            "rule_atoms={rule_atoms} update_atoms={update_atoms} update_ids={update_ids} max_atoms_per_expression={max_atoms_per_expression} modifiers={modifiers:?} selectors={selector_kinds:?} directive_charged={directive_charged} target_charged={target_charged} token_size={} directive_record_size={} target_record_size={} target_atom_size={} id_selector_size={}",
            size_of::<crate::seclang::Token<'_>>(),
            (directive_charged - update_ids * size_of::<crate::seclang::Token<'_>>()) / directive_count,
            size_of::<TargetRecord>(),
            size_of::<TargetAtom<'_>>(),
            size_of::<super::IdSelector<'_>>(),
        );
        assert_eq!(rule_atoms, 1_735);
        assert_eq!(max_atoms_per_expression, 10);
        assert_eq!(modifiers, [1_656, 28, 51]);
        assert_eq!(selector_kinds, [946, 432, 348, 9]);
        assert_eq!(update_atoms, 55);
        assert_eq!(update_ids, 55);
    }

    fn collect_conf_files(directory: &Path, files: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(directory).expect("configured CRS rules directory must be readable") {
            let entry = entry.expect("CRS directory entry must be readable");
            let path = entry.path();
            if path.is_dir() {
                collect_conf_files(&path, files);
            } else if path.extension().is_some_and(|extension| extension == "conf") {
                files.push(path);
            }
        }
    }
}
