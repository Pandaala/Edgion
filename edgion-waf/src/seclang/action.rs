use std::mem::size_of;
use std::ops::Range;

use thiserror::Error;

use super::operator::OperatorSyntaxDocument;
use super::{Directive, LogicalLine, SourceDocument, SourcePosition, SourceSpan, Token};

const DEFAULT_MAX_ACTION_LISTS: usize = 100_000;
const DEFAULT_MAX_ENTRIES_PER_LIST: usize = 1_024;
const DEFAULT_MAX_ACTION_ENTRIES: usize = 200_000;
const DEFAULT_MAX_SCANNED_ACTION_BYTES: usize = 16 * 1024 * 1024;
const DEFAULT_MAX_ACTION_OWNED_BYTES: usize = 32 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActionSyntaxLimits {
    pub max_lists: usize,
    pub max_entries_per_list: usize,
    pub max_entries: usize,
    pub max_scanned_bytes: usize,
    pub max_owned_bytes: usize,
}

impl Default for ActionSyntaxLimits {
    fn default() -> Self {
        Self {
            max_lists: DEFAULT_MAX_ACTION_LISTS,
            max_entries_per_list: DEFAULT_MAX_ENTRIES_PER_LIST,
            max_entries: DEFAULT_MAX_ACTION_ENTRIES,
            max_scanned_bytes: DEFAULT_MAX_SCANNED_ACTION_BYTES,
            max_owned_bytes: DEFAULT_MAX_ACTION_OWNED_BYTES,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionListKind {
    Rule,
    Action,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionEntryKind {
    Action,
    Transformation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionValueSpelling {
    Unquoted,
    SingleQuoted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActionText<'source> {
    text: &'source str,
    logical_start: usize,
    logical_end: usize,
    span: SourceSpan,
}

impl<'source> ActionText<'source> {
    pub fn text(&self) -> &'source str {
        self.text
    }

    pub fn logical_range(&self) -> Range<usize> {
        self.logical_start..self.logical_end
    }

    pub fn span(&self) -> SourceSpan {
        self.span
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActionValue<'source> {
    spelling: ActionValueSpelling,
    envelope: ActionText<'source>,
    body: ActionText<'source>,
}

impl<'source> ActionValue<'source> {
    pub fn spelling(&self) -> ActionValueSpelling {
        self.spelling
    }

    pub fn envelope(&self) -> ActionText<'source> {
        self.envelope
    }

    pub fn body(&self) -> ActionText<'source> {
        self.body
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActionEntry<'source> {
    full: ActionText<'source>,
    name: ActionText<'source>,
    value: Option<ActionValue<'source>>,
    kind: ActionEntryKind,
}

impl<'source> ActionEntry<'source> {
    pub fn full(&self) -> ActionText<'source> {
        self.full
    }

    pub fn name(&self) -> ActionText<'source> {
        self.name
    }

    pub fn value(&self) -> Option<ActionValue<'source>> {
        self.value
    }

    pub fn kind(&self) -> ActionEntryKind {
        self.kind
    }
}

#[derive(Debug, PartialEq, Eq)]
enum ActionMapping {
    None,
    List(usize),
}

#[derive(Debug, PartialEq, Eq)]
struct ActionListRecord<'source> {
    logical_line_index: usize,
    kind: ActionListKind,
    content: &'source str,
    logical_start: usize,
    span: SourceSpan,
    entry_start: usize,
    entry_count: usize,
}

#[derive(Debug)]
pub(super) struct ActionSyntaxDocument<'source> {
    operators: OperatorSyntaxDocument<'source>,
    mappings: Vec<ActionMapping>,
    lists: Vec<ActionListRecord<'source>>,
    entries: Vec<ActionEntry<'source>>,
    charged_bytes: usize,
    scanned_bytes: usize,
}

impl<'source> ActionSyntaxDocument<'source> {
    pub(super) fn source(&self) -> &'source SourceDocument {
        self.operators.source()
    }

    pub(super) fn charged_bytes(&self) -> usize {
        self.charged_bytes
    }

    pub(super) fn retained_charged_bytes(&self) -> usize {
        self.operators
            .retained_charged_bytes()
            .and_then(|bytes| bytes.checked_add(self.charged_bytes))
            .expect("published action syntax charge must fit after checked parser arithmetic")
    }

    pub(super) fn list_count(&self) -> usize {
        self.lists.len()
    }

    pub(super) fn entry_count(&self) -> usize {
        self.entries.len()
    }

    pub(super) fn mapping_count(&self) -> usize {
        self.mappings.len()
    }

    pub(super) fn get(&self, index: usize) -> Option<ActionMappingView<'_, 'source>> {
        match *self.mappings.get(index)? {
            ActionMapping::None => Some(ActionMappingView::None),
            ActionMapping::List(list) => {
                let record = self.lists.get(list).expect("private action mapping must remain valid");
                Some(ActionMappingView::List(ActionListView {
                    entries: checked_slice(
                        &self.entries,
                        record.entry_start,
                        record.entry_count,
                        "private action entry range must remain valid",
                    ),
                    record,
                }))
            }
        }
    }

    pub(super) fn operators(&self) -> &OperatorSyntaxDocument<'source> {
        &self.operators
    }
}

fn checked_slice<'a, T>(values: &'a [T], start: usize, count: usize, reason: &'static str) -> &'a [T] {
    let end = start.checked_add(count).expect(reason);
    values.get(start..end).expect(reason)
}

#[derive(Debug, Clone, Copy)]
#[allow(clippy::large_enum_variant)] // Short-lived views stay allocation-free and borrow the flat arena.
pub enum ActionMappingView<'view, 'source> {
    None,
    List(ActionListView<'view, 'source>),
}

#[derive(Debug, Clone, Copy)]
pub struct ActionListView<'view, 'source> {
    record: &'view ActionListRecord<'source>,
    entries: &'view [ActionEntry<'source>],
}

impl<'view, 'source> ActionListView<'view, 'source> {
    pub fn logical_line_index(&self) -> usize {
        self.record.logical_line_index
    }

    pub fn kind(&self) -> ActionListKind {
        self.record.kind
    }

    pub fn content(&self) -> &'source str {
        self.record.content
    }

    pub fn logical_range(&self) -> Range<usize> {
        self.record.logical_start..self.record.logical_start + self.record.content.len()
    }

    pub fn span(&self) -> SourceSpan {
        self.record.span
    }

    pub fn entries(&self) -> &'view [ActionEntry<'source>] {
        self.entries
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmptyActionKind {
    Entry,
    Name,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ActionSyntaxError {
    #[error("operator wrapper has {directives} directives, {operator_mappings} operator mappings, and {target_records} target records")]
    InheritedRecordMismatch {
        directives: usize,
        operator_mappings: usize,
        target_records: usize,
    },
    #[error("action mapping metadata requires more than {limit} owned bytes")]
    MappingOwnedByteLimit { limit: usize },
    #[error("action-list count exceeds the configured limit of {limit} at {position:?}")]
    ListLimit { limit: usize, position: SourcePosition },
    #[error("action lists exceed the configured scanned-byte limit of {limit} at {position:?}")]
    ScannedByteLimit { limit: usize, position: SourcePosition },
    #[error("action list exceeds its entry limit of {limit} at {position:?}")]
    ListEntryLimit { limit: usize, position: SourcePosition },
    #[error("action document exceeds its entry limit of {limit} at {position:?}")]
    DocumentEntryLimit { limit: usize, position: SourcePosition },
    #[error("action syntax contains an empty {kind:?} at {position:?}")]
    Empty {
        kind: EmptyActionKind,
        position: SourcePosition,
    },
    #[error("single-quoted action value has no closing quote at {position:?}")]
    UnclosedQuote { position: SourcePosition },
    #[error("single-quoted action value has a non-whitespace suffix at {position:?}")]
    QuotedValueSuffix { position: SourcePosition },
    #[error("action syntax requires more than {limit} charged owned bytes at {position:?}")]
    OwnedByteLimit { limit: usize, position: SourcePosition },
    #[error("cannot map action syntax on logical line {logical_line} at byte offset {logical_offset}")]
    PositionMapping { logical_line: usize, logical_offset: usize },
    #[error("cannot allocate bounded SecLang {operation}; reduce the source or configured limits")]
    Allocation { operation: &'static str },
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum ActionBuildError {
    Syntax(ActionSyntaxError),
    RetainedByteLimit,
}

impl From<ActionSyntaxError> for ActionBuildError {
    fn from(error: ActionSyntaxError) -> Self {
        Self::Syntax(error)
    }
}

pub(super) fn parse_action_syntax<'source>(
    operators: OperatorSyntaxDocument<'source>,
    limits: ActionSyntaxLimits,
    max_retained_bytes: usize,
) -> Result<ActionSyntaxDocument<'source>, ActionBuildError> {
    let directive_count = operators.directives().len();
    let operator_mappings = operators.mapping_count();
    let target_records = operators.targets().record_count();
    if directive_count != operator_mappings || directive_count != target_records {
        return Err(ActionSyntaxError::InheritedRecordMismatch {
            directives: directive_count,
            operator_mappings,
            target_records,
        }
        .into());
    }
    let inherited = operators
        .retained_charged_bytes()
        .ok_or(ActionBuildError::RetainedByteLimit)?;
    let mapping_bytes =
        size_of::<ActionMapping>()
            .checked_mul(directive_count)
            .ok_or(ActionSyntaxError::MappingOwnedByteLimit {
                limit: limits.max_owned_bytes,
            })?;
    if mapping_bytes > limits.max_owned_bytes {
        return Err(ActionSyntaxError::MappingOwnedByteLimit {
            limit: limits.max_owned_bytes,
        }
        .into());
    }
    ensure_cumulative(inherited, mapping_bytes, max_retained_bytes)?;
    let mut mappings = Vec::new();
    mappings
        .try_reserve_exact(directive_count)
        .map_err(|_| ActionSyntaxError::Allocation {
            operation: "action mapping records",
        })?;
    let mut document = ActionSyntaxDocument {
        operators,
        mappings,
        lists: Vec::new(),
        entries: Vec::new(),
        charged_bytes: mapping_bytes,
        scanned_bytes: 0,
    };

    for directive in document.operators.directives().iter() {
        let present = match directive.directive() {
            Directive::SecRule { actions, .. } => actions.map(|token| (ActionListKind::Rule, token)),
            Directive::SecAction { actions } => Some((ActionListKind::Action, actions)),
            _ => None,
        };
        let Some((kind, token)) = present else {
            document.mappings.push(ActionMapping::None);
            continue;
        };
        if document.lists.len() >= limits.max_lists {
            return Err(ActionSyntaxError::ListLimit {
                limit: limits.max_lists,
                position: token.content_span().start,
            }
            .into());
        }
        let next_scanned =
            document
                .scanned_bytes
                .checked_add(token.value().len())
                .ok_or(ActionSyntaxError::ScannedByteLimit {
                    limit: limits.max_scanned_bytes,
                    position: token.content_span().start,
                })?;
        if next_scanned > limits.max_scanned_bytes {
            return Err(ActionSyntaxError::ScannedByteLimit {
                limit: limits.max_scanned_bytes,
                position: token.content_span().start,
            }
            .into());
        }
        let line_index = directive.logical_line_index();
        let line = document.operators.directives().source().lines().get(line_index).ok_or(
            ActionSyntaxError::PositionMapping {
                logical_line: line_index + 1,
                logical_offset: token.content_logical_range().start,
            },
        )?;
        parse_list(
            kind,
            token,
            line,
            line_index,
            inherited,
            limits,
            max_retained_bytes,
            &mut document.lists,
            &mut document.entries,
            &mut document.charged_bytes,
        )?;
        document.scanned_bytes = next_scanned;
        document.mappings.push(ActionMapping::List(document.lists.len() - 1));
    }
    Ok(document)
}

// Independent inputs: one call site; token source mapping, inherited count,
// limits and the document's three output arenas are threaded by no other function.
#[allow(clippy::too_many_arguments)]
fn parse_list<'source>(
    kind: ActionListKind,
    token: Token<'source>,
    line: &'source LogicalLine,
    line_index: usize,
    inherited: usize,
    limits: ActionSyntaxLimits,
    max_retained_bytes: usize,
    lists: &mut Vec<ActionListRecord<'source>>,
    entries: &mut Vec<ActionEntry<'source>>,
    charged_bytes: &mut usize,
) -> Result<(), ActionBuildError> {
    let source = token.value();
    let entry_start = entries.len();
    if !source.is_empty() {
        let mut candidate_start = 0;
        let mut list_count = 0;
        loop {
            if list_count >= limits.max_entries_per_list {
                return Err(ActionSyntaxError::ListEntryLimit {
                    limit: limits.max_entries_per_list,
                    position: map_start(line, line_index, token, candidate_start)?,
                }
                .into());
            }
            if entries.len() >= limits.max_entries {
                return Err(ActionSyntaxError::DocumentEntryLimit {
                    limit: limits.max_entries,
                    position: map_start(line, line_index, token, candidate_start)?,
                }
                .into());
            }
            let scanned = scan_candidate(source, candidate_start, line, line_index, token)?;
            let entry = build_entry(source, candidate_start, scanned, line, line_index, token)?;
            charge_and_reserve_entry(entry, inherited, limits, max_retained_bytes, entries, charged_bytes)?;
            list_count = list_count.checked_add(1).ok_or(ActionSyntaxError::ListEntryLimit {
                limit: limits.max_entries_per_list,
                position: map_start(line, line_index, token, candidate_start)?,
            })?;
            if scanned.end == source.len() {
                break;
            }
            candidate_start = scanned.end.checked_add(1).ok_or(ActionSyntaxError::PositionMapping {
                logical_line: line_index + 1,
                logical_offset: scanned.end,
            })?;
        }
    }

    let list_record = ActionListRecord {
        logical_line_index: line_index,
        kind,
        content: source,
        logical_start: token.content_logical_range().start,
        span: token.content_span(),
        entry_start,
        entry_count: entries.len() - entry_start,
    };
    charge_owned(
        charged_bytes,
        size_of::<ActionListRecord<'source>>(),
        limits.max_owned_bytes,
        token.content_span().start,
    )?;
    ensure_cumulative(inherited, *charged_bytes, max_retained_bytes)?;
    lists.try_reserve(1).map_err(|_| ActionSyntaxError::Allocation {
        operation: "action-list records",
    })?;
    lists.push(list_record);
    Ok(())
}

#[derive(Clone, Copy)]
struct ScannedCandidate {
    end: usize,
    colon: Option<usize>,
    quote: Option<(usize, usize)>,
}

fn scan_candidate(
    source: &str,
    start: usize,
    line: &LogicalLine,
    line_index: usize,
    token: Token<'_>,
) -> Result<ScannedCandidate, ActionSyntaxError> {
    let bytes = source.as_bytes();
    if start == bytes.len() {
        return Ok(ScannedCandidate {
            end: start,
            colon: None,
            quote: None,
        });
    }
    let mut cursor = start;
    let mut backslashes = 0_usize;
    let mut colon = None;
    let mut value_started = false;
    let mut quote_start = None;
    let mut quote_end = None;
    while cursor < bytes.len() {
        let byte = bytes[cursor];
        let exposed = backslashes.is_multiple_of(2);
        if let Some(open) = quote_start {
            if quote_end.is_none() {
                if byte == b'\'' && exposed {
                    quote_end = Some(cursor);
                }
            } else if let Some(close) = quote_end {
                if byte == b',' && exposed {
                    return Ok(ScannedCandidate {
                        end: cursor,
                        colon,
                        quote: Some((open, close)),
                    });
                }
                if !matches!(byte, b' ' | b'\t') {
                    return Err(ActionSyntaxError::QuotedValueSuffix {
                        position: map_start(line, line_index, token, cursor)?,
                    });
                }
            }
        } else if colon.is_none() {
            if byte == b':' && exposed {
                colon = Some(cursor);
            } else if byte == b',' && exposed {
                return Ok(ScannedCandidate {
                    end: cursor,
                    colon: None,
                    quote: None,
                });
            }
        } else if !value_started {
            if matches!(byte, b' ' | b'\t') {
            } else {
                value_started = true;
                if byte == b'\'' {
                    quote_start = Some(cursor);
                } else if byte == b',' && exposed {
                    return Ok(ScannedCandidate {
                        end: cursor,
                        colon,
                        quote: None,
                    });
                }
            }
        } else if byte == b',' && exposed {
            return Ok(ScannedCandidate {
                end: cursor,
                colon,
                quote: None,
            });
        }
        if byte == b'\\' {
            backslashes = backslashes.checked_add(1).ok_or(ActionSyntaxError::PositionMapping {
                logical_line: line_index + 1,
                logical_offset: cursor,
            })?;
        } else {
            backslashes = 0;
        }
        cursor += 1;
    }
    if let Some(open) = quote_start {
        let Some(close) = quote_end else {
            return Err(ActionSyntaxError::UnclosedQuote {
                position: map_start(line, line_index, token, open)?,
            });
        };
        Ok(ScannedCandidate {
            end: bytes.len(),
            colon,
            quote: Some((open, close)),
        })
    } else {
        Ok(ScannedCandidate {
            end: bytes.len(),
            colon,
            quote: None,
        })
    }
}

fn build_entry<'source>(
    source: &'source str,
    start: usize,
    scanned: ScannedCandidate,
    line: &LogicalLine,
    line_index: usize,
    token: Token<'source>,
) -> Result<ActionEntry<'source>, ActionSyntaxError> {
    let full_range = trim_sp_htab(source.as_bytes(), start..scanned.end);
    if full_range.is_empty() {
        return Err(ActionSyntaxError::Empty {
            kind: EmptyActionKind::Entry,
            position: map_start(line, line_index, token, start)?,
        });
    }
    let name_range = trim_sp_htab(
        source.as_bytes(),
        full_range.start..scanned.colon.unwrap_or(full_range.end),
    );
    if name_range.is_empty() {
        return Err(ActionSyntaxError::Empty {
            kind: EmptyActionKind::Name,
            position: map_start(line, line_index, token, name_range.start)?,
        });
    }
    let name = action_text(source, name_range.clone(), line, line_index, token)?;
    let value = if let Some(colon) = scanned.colon {
        let envelope_range = trim_sp_htab(source.as_bytes(), colon + 1..full_range.end);
        let envelope = action_text(source, envelope_range.clone(), line, line_index, token)?;
        let (spelling, body_range) = match scanned.quote {
            Some((open, close)) => (ActionValueSpelling::SingleQuoted, open + 1..close),
            None => (ActionValueSpelling::Unquoted, envelope_range),
        };
        Some(ActionValue {
            spelling,
            envelope,
            body: action_text(source, body_range, line, line_index, token)?,
        })
    } else {
        None
    };
    Ok(ActionEntry {
        full: action_text(source, full_range, line, line_index, token)?,
        name,
        value,
        kind: if name.text.eq_ignore_ascii_case("t") {
            ActionEntryKind::Transformation
        } else {
            ActionEntryKind::Action
        },
    })
}

fn charge_and_reserve_entry<'source>(
    entry: ActionEntry<'source>,
    inherited: usize,
    limits: ActionSyntaxLimits,
    max_retained_bytes: usize,
    entries: &mut Vec<ActionEntry<'source>>,
    charged_bytes: &mut usize,
) -> Result<(), ActionBuildError> {
    charge_owned(
        charged_bytes,
        size_of::<ActionEntry<'source>>(),
        limits.max_owned_bytes,
        entry.full.span.start,
    )?;
    ensure_cumulative(inherited, *charged_bytes, max_retained_bytes)?;
    entries.try_reserve(1).map_err(|_| ActionSyntaxError::Allocation {
        operation: "action entry records",
    })?;
    entries.push(entry);
    Ok(())
}

fn action_text<'source>(
    source: &'source str,
    range: Range<usize>,
    line: &LogicalLine,
    line_index: usize,
    token: Token<'source>,
) -> Result<ActionText<'source>, ActionSyntaxError> {
    let base = token.content_logical_range().start;
    let logical_start = base
        .checked_add(range.start)
        .ok_or(ActionSyntaxError::PositionMapping {
            logical_line: line_index + 1,
            logical_offset: base,
        })?;
    let logical_end = base.checked_add(range.end).ok_or(ActionSyntaxError::PositionMapping {
        logical_line: line_index + 1,
        logical_offset: base,
    })?;
    Ok(ActionText {
        text: source.get(range).ok_or(ActionSyntaxError::PositionMapping {
            logical_line: line_index + 1,
            logical_offset: logical_start,
        })?,
        logical_start,
        logical_end,
        span: map_span(line, line_index, logical_start, logical_end)?,
    })
}

fn map_start(
    line: &LogicalLine,
    line_index: usize,
    token: Token<'_>,
    relative: usize,
) -> Result<SourcePosition, ActionSyntaxError> {
    let base = token.content_logical_range().start;
    let offset = base.checked_add(relative).ok_or(ActionSyntaxError::PositionMapping {
        logical_line: line_index + 1,
        logical_offset: base,
    })?;
    line.position_at(offset).ok_or(ActionSyntaxError::PositionMapping {
        logical_line: line_index + 1,
        logical_offset: offset,
    })
}

fn map_span(line: &LogicalLine, line_index: usize, start: usize, end: usize) -> Result<SourceSpan, ActionSyntaxError> {
    let start_position = line.position_at(start).ok_or(ActionSyntaxError::PositionMapping {
        logical_line: line_index + 1,
        logical_offset: start,
    })?;
    let end_position = if start == end {
        start_position
    } else {
        line.position_at_end(end).ok_or(ActionSyntaxError::PositionMapping {
            logical_line: line_index + 1,
            logical_offset: end,
        })?
    };
    Ok(SourceSpan {
        start: start_position,
        end: end_position,
    })
}

fn trim_sp_htab(bytes: &[u8], mut range: Range<usize>) -> Range<usize> {
    while range.start < range.end && matches!(bytes[range.start], b' ' | b'\t') {
        range.start += 1;
    }
    while range.end > range.start && matches!(bytes[range.end - 1], b' ' | b'\t') {
        range.end -= 1;
    }
    range
}

fn charge_owned(
    charged: &mut usize,
    amount: usize,
    limit: usize,
    position: SourcePosition,
) -> Result<(), ActionSyntaxError> {
    let next = charged
        .checked_add(amount)
        .ok_or(ActionSyntaxError::OwnedByteLimit { limit, position })?;
    if next > limit {
        return Err(ActionSyntaxError::OwnedByteLimit { limit, position });
    }
    *charged = next;
    Ok(())
}

fn ensure_cumulative(inherited: usize, action: usize, limit: usize) -> Result<(), ActionBuildError> {
    let total = inherited
        .checked_add(action)
        .ok_or(ActionBuildError::RetainedByteLimit)?;
    if total > limit {
        Err(ActionBuildError::RetainedByteLimit)
    } else {
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../tests/unit/seclang/action_test.rs"]
mod tests;
