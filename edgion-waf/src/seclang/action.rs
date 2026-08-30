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

    #[cfg(test)]
    fn iter(&self) -> impl Iterator<Item = ActionMappingView<'_, 'source>> {
        self.mappings.iter().map(|mapping| match *mapping {
            ActionMapping::None => ActionMappingView::None,
            ActionMapping::List(index) => {
                let record = self
                    .lists
                    .get(index)
                    .expect("private action mapping must remain valid in an immutable document");
                let entries = checked_slice(
                    &self.entries,
                    record.entry_start,
                    record.entry_count,
                    "private action entry range must remain valid",
                );
                ActionMappingView::List(ActionListView { record, entries })
            }
        })
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

    #[cfg(test)]
    pub(super) fn operators_mut_for_test(&mut self) -> &mut OperatorSyntaxDocument<'source> {
        &mut self.operators
    }

    #[cfg(test)]
    pub(super) fn swap_mappings_for_test(&mut self, left: usize, right: usize) {
        self.mappings.swap(left, right);
    }

    #[cfg(test)]
    pub(super) fn set_first_list_kind_for_test(&mut self, kind: ActionListKind) {
        if let Some(list) = self.lists.first_mut() {
            list.kind = kind;
        }
    }

    #[cfg(test)]
    pub(super) fn set_first_mapping_none_for_test(&mut self) {
        if let Some(mapping) = self.mappings.first_mut() {
            *mapping = ActionMapping::None;
        }
    }

    #[cfg(test)]
    fn lists(&self) -> impl Iterator<Item = ActionListView<'_, 'source>> {
        self.lists.iter().map(|record| ActionListView {
            entries: checked_slice(
                &self.entries,
                record.entry_start,
                record.entry_count,
                "private action entry range must remain valid",
            ),
            record,
        })
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
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::mem::size_of;
    use std::path::{Path, PathBuf};

    use super::{
        parse_action_syntax, ActionEntry, ActionEntryKind, ActionListKind, ActionListRecord, ActionMapping,
        ActionMappingView, ActionSyntaxDocument, ActionSyntaxError, ActionSyntaxLimits, ActionValueSpelling,
        EmptyActionKind,
    };
    use crate::seclang::operator::{parse_operator_syntax, OperatorSyntaxLimits};
    use crate::seclang::target::{parse_target_syntax, TargetSyntaxLimits};
    use crate::seclang::{
        parse_directives, scan_source, tokenize_document, DirectiveLimits, SourceLimits, TokenizerLimits,
    };

    fn with_actions(source: &[u8], inspect: impl FnOnce(&ActionSyntaxDocument<'_>)) {
        let source = scan_source("test", source, SourceLimits::default()).unwrap();
        let tokenized = tokenize_document(&source, TokenizerLimits::default()).unwrap();
        let directives = parse_directives(&tokenized, DirectiveLimits::default()).unwrap();
        let targets = parse_target_syntax(&directives, TargetSyntaxLimits::default()).unwrap();
        let operators = parse_operator_syntax(directives, targets, OperatorSyntaxLimits::default()).unwrap();
        drop(tokenized);
        let actions = parse_action_syntax(operators, ActionSyntaxLimits::default(), usize::MAX).unwrap();
        inspect(&actions);
    }

    fn action_error(source: &[u8], limits: ActionSyntaxLimits) -> ActionSyntaxError {
        let source = scan_source("test", source, SourceLimits::default()).unwrap();
        let tokenized = tokenize_document(&source, TokenizerLimits::default()).unwrap();
        let directives = parse_directives(&tokenized, DirectiveLimits::default()).unwrap();
        let targets = parse_target_syntax(&directives, TargetSyntaxLimits::default()).unwrap();
        let operators = parse_operator_syntax(directives, targets, OperatorSyntaxLimits::default()).unwrap();
        match parse_action_syntax(operators, limits, usize::MAX).unwrap_err() {
            super::ActionBuildError::Syntax(error) => error,
            super::ActionBuildError::RetainedByteLimit => panic!("unexpected retained limit"),
        }
    }

    fn with_operator_document(
        source: &[u8],
        inspect: impl FnOnce(crate::seclang::operator::OperatorSyntaxDocument<'_>),
    ) {
        let source = scan_source("test", source, SourceLimits::default()).unwrap();
        let tokenized = tokenize_document(&source, TokenizerLimits::default()).unwrap();
        let directives = parse_directives(&tokenized, DirectiveLimits::default()).unwrap();
        let targets = parse_target_syntax(&directives, TargetSyntaxLimits::default()).unwrap();
        inspect(parse_operator_syntax(directives, targets, OperatorSyntaxLimits::default()).unwrap());
    }

    #[test]
    fn distinguishes_absent_empty_rule_and_action_lists() {
        with_actions(
            b"SecRule ARGS \"@rx x\"\nSecRule ARGS \"@rx x\" \"\"\nSecAction \"deny\"",
            |document| {
                let mappings = document.iter().collect::<Vec<_>>();
                assert!(matches!(mappings[0], ActionMappingView::None));
                let ActionMappingView::List(empty) = mappings[1] else {
                    panic!("expected explicit empty list")
                };
                assert_eq!(empty.kind(), ActionListKind::Rule);
                assert!(empty.entries().is_empty());
                let ActionMappingView::List(action) = mappings[2] else {
                    panic!("expected SecAction list")
                };
                assert_eq!(action.kind(), ActionListKind::Action);
                assert_eq!(action.entries()[0].name().text(), "deny");
                assert_eq!(document.list_count(), 2);
                assert_eq!(document.entry_count(), 1);
            },
        );
    }

    #[test]
    fn preserves_value_presence_spelling_case_duplicates_and_first_colon() {
        with_actions(
            br#"SecAction "Deny,id:1,tag:,msg:'a,b:c',empty:'',setvar:tx.a:1,T:none,t:lowercase,deny""#,
            |document| {
                let entries = document.lists().next().unwrap().entries();
                assert_eq!(entries.len(), 9);
                assert!(entries[0].value().is_none());
                assert_eq!(entries[2].value().unwrap().body().text(), "");
                assert_eq!(entries[2].value().unwrap().spelling(), ActionValueSpelling::Unquoted);
                assert_eq!(entries[3].value().unwrap().body().text(), "a,b:c");
                assert_eq!(
                    entries[3].value().unwrap().spelling(),
                    ActionValueSpelling::SingleQuoted
                );
                assert_eq!(entries[4].value().unwrap().body().text(), "");
                assert_eq!(entries[5].value().unwrap().body().text(), "tx.a:1");
                assert_eq!(entries[6].name().text(), "T");
                assert_eq!(entries[6].kind(), ActionEntryKind::Transformation);
                assert_eq!(entries[7].kind(), ActionEntryKind::Transformation);
                assert_eq!(entries[8].name().text(), "deny");
            },
        );
    }

    #[test]
    fn trims_only_ascii_space_and_tab_and_keeps_macros_and_escapes() {
        with_actions(
            "SecAction \"  msg:  '%{tx.a}\\\"x'  , tag:\u{a0}x\u{a0} ,deny  \"".as_bytes(),
            |document| {
                let entries = document.lists().next().unwrap().entries();
                assert_eq!(entries[0].full().text(), "msg:  '%{tx.a}\\\"x'");
                assert_eq!(entries[0].value().unwrap().body().text(), "%{tx.a}\\\"x");
                assert_eq!(entries[1].value().unwrap().body().text(), "\u{a0}x\u{a0}");
                assert_eq!(entries[2].name().text(), "deny");
            },
        );
    }

    #[test]
    fn delimiter_backslash_parity_is_exact_for_one_through_five() {
        for slashes in 1..=5 {
            let run = "\\".repeat(slashes);
            let source = format!(r#"SecAction "msg:a{run},b,deny""#);
            with_actions(source.as_bytes(), |document| {
                let entries = document.lists().next().unwrap().entries();
                if slashes % 2 == 1 {
                    assert_eq!(entries.len(), 2, "slashes={slashes}");
                    assert_eq!(entries[0].value().unwrap().body().text(), format!("a{run},b"));
                } else {
                    assert_eq!(entries.len(), 3, "slashes={slashes}");
                }
            });

            let source = format!(r#"SecAction "name{run}:value,deny""#);
            with_actions(source.as_bytes(), |document| {
                let first = document.lists().next().unwrap().entries()[0];
                assert_eq!(first.value().is_some(), slashes % 2 == 0, "slashes={slashes}");
            });

            let source = format!(r#"SecAction "msg:'a{run}' ,deny""#);
            if slashes % 2 == 0 {
                with_actions(source.as_bytes(), |_| {});
            } else {
                assert!(matches!(
                    action_error(source.as_bytes(), ActionSyntaxLimits::default()),
                    ActionSyntaxError::UnclosedQuote { .. }
                ));
            }
        }
    }

    #[test]
    fn unquoted_quote_is_ordinary_and_quoted_suffix_fails_closed() {
        with_actions(br#"SecAction "msg:a'b,deny""#, |document| {
            let entries = document.lists().next().unwrap().entries();
            assert_eq!(entries.len(), 2);
            assert_eq!(entries[0].value().unwrap().body().text(), "a'b");
        });
        assert!(matches!(
            action_error(br#"SecAction "msg:'abc""#, ActionSyntaxLimits::default()),
            ActionSyntaxError::UnclosedQuote { .. }
        ));
        assert!(matches!(
            action_error(br#"SecAction "msg:'abc'x""#, ActionSyntaxLimits::default()),
            ActionSyntaxError::QuotedValueSuffix { .. }
        ));
    }

    #[test]
    fn rejects_leading_trailing_consecutive_empty_entries_and_empty_names() {
        for source in [
            br#"SecAction ",deny""#.as_slice(),
            br#"SecAction "deny,""#,
            br#"SecAction "deny,,log""#,
        ] {
            assert!(matches!(
                action_error(source, ActionSyntaxLimits::default()),
                ActionSyntaxError::Empty {
                    kind: EmptyActionKind::Entry,
                    ..
                }
            ));
        }
        assert!(matches!(
            action_error(br#"SecAction " :value""#, ActionSyntaxLimits::default()),
            ActionSyntaxError::Empty {
                kind: EmptyActionKind::Name,
                ..
            }
        ));
    }

    #[test]
    fn retains_continuation_positions_ranges_and_source_after_tokenizer_drop() {
        let source = scan_source(
            "actions.conf",
            b"SecAction \\\n              \"msg:'caf\xc3\xa9',deny\"",
            SourceLimits::default(),
        )
        .unwrap();
        let tokenized = tokenize_document(&source, TokenizerLimits::default()).unwrap();
        let directives = parse_directives(&tokenized, DirectiveLimits::default()).unwrap();
        let targets = parse_target_syntax(&directives, TargetSyntaxLimits::default()).unwrap();
        let operators = parse_operator_syntax(directives, targets, OperatorSyntaxLimits::default()).unwrap();
        drop(tokenized);
        let document = parse_action_syntax(operators, ActionSyntaxLimits::default(), usize::MAX).unwrap();
        let list = document.lists().next().unwrap();
        assert_eq!(list.logical_line_index(), 0);
        assert_eq!(list.span().start.line, 2);
        assert_eq!(list.logical_range(), 11..27);
        assert_eq!(list.entries()[0].value().unwrap().body().text(), "caf\u{e9}");
        assert_eq!(document.source().source_id(), "actions.conf");

        let source = scan_source(
            "joined-action.conf",
            b"SecAction \"msg:'caf\\\n\xc3\xa9',deny\"",
            SourceLimits::default(),
        )
        .unwrap();
        let tokenized = tokenize_document(&source, TokenizerLimits::default()).unwrap();
        let directives = parse_directives(&tokenized, DirectiveLimits::default()).unwrap();
        let targets = parse_target_syntax(&directives, TargetSyntaxLimits::default()).unwrap();
        let operators = parse_operator_syntax(directives, targets, OperatorSyntaxLimits::default()).unwrap();
        let document = parse_action_syntax(operators, ActionSyntaxLimits::default(), usize::MAX).unwrap();
        let value = document.lists().next().unwrap().entries()[0].value().unwrap().body();
        assert_eq!(value.text(), "caf\u{e9}");
        assert_eq!(value.span().start.line, 1);
        assert_eq!(value.span().end.line, 2);
    }

    #[test]
    fn enforces_exact_list_scanned_entry_and_owned_limits_in_precedence_order() {
        let base = ActionSyntaxLimits::default();
        assert!(matches!(
            action_error(br#"SecAction "deny""#, ActionSyntaxLimits { max_lists: 0, ..base }),
            ActionSyntaxError::ListLimit { .. }
        ));
        assert!(matches!(
            action_error(
                br#"SecAction "deny""#,
                ActionSyntaxLimits {
                    max_scanned_bytes: 3,
                    max_entries_per_list: 0,
                    ..base
                }
            ),
            ActionSyntaxError::ScannedByteLimit { .. }
        ));
        assert!(matches!(
            action_error(
                br#"SecAction "deny,""#,
                ActionSyntaxLimits {
                    max_entries_per_list: 1,
                    max_entries: 1,
                    ..base
                }
            ),
            ActionSyntaxError::ListEntryLimit { .. }
        ));
        assert!(matches!(
            action_error(
                br#"SecAction "deny,log""#,
                ActionSyntaxLimits {
                    max_entries_per_list: 2,
                    max_entries: 1,
                    ..base
                }
            ),
            ActionSyntaxError::DocumentEntryLimit { .. }
        ));
        assert!(matches!(
            action_error(
                br#"SecAction "deny""#,
                ActionSyntaxLimits {
                    max_owned_bytes: 0,
                    ..base
                }
            ),
            ActionSyntaxError::MappingOwnedByteLimit { .. }
        ));
    }

    #[test]
    fn accepts_every_exact_limit_and_rejects_each_first_excess_charge() {
        let expected_owned = size_of::<ActionMapping>() + size_of::<ActionEntry>() + size_of::<ActionListRecord>();
        with_operator_document(br#"SecAction "deny""#, |operators| {
            let inherited = operators.retained_charged_bytes().unwrap();
            let document = parse_action_syntax(
                operators,
                ActionSyntaxLimits {
                    max_lists: 1,
                    max_entries_per_list: 1,
                    max_entries: 1,
                    max_scanned_bytes: 4,
                    max_owned_bytes: expected_owned,
                },
                inherited + expected_owned,
            )
            .unwrap();
            assert_eq!(document.charged_bytes(), expected_owned);
        });

        with_operator_document(br#"SecAction "deny""#, |operators| {
            let inherited = operators.retained_charged_bytes().unwrap();
            assert!(matches!(
                parse_action_syntax(
                    operators,
                    ActionSyntaxLimits::default(),
                    inherited + size_of::<ActionMapping>() - 1,
                ),
                Err(super::ActionBuildError::RetainedByteLimit)
            ));
        });
        assert!(matches!(
            action_error(
                br#"SecAction "deny""#,
                ActionSyntaxLimits {
                    max_owned_bytes: expected_owned - 1,
                    ..ActionSyntaxLimits::default()
                }
            ),
            ActionSyntaxError::OwnedByteLimit { .. }
        ));
        assert!(matches!(
            super::ensure_cumulative(usize::MAX, 1, usize::MAX),
            Err(super::ActionBuildError::RetainedByteLimit)
        ));
    }

    #[test]
    fn charged_sizes_are_exact_and_errors_do_not_echo_content() {
        with_actions(br#"SecAction "deny,msg:'secret-marker'""#, |document| {
            assert_eq!(
                document.charged_bytes(),
                size_of::<ActionMapping>() + size_of::<ActionListRecord>() + 2 * size_of::<ActionEntry>()
            );
            assert!(document.retained_charged_bytes() >= document.charged_bytes());
        });
        let error = action_error(br#"SecAction "secret-marker,""#, ActionSyntaxLimits::default());
        assert!(!error.to_string().contains("secret-marker"));
        assert!(!format!("{error:?}").contains("secret-marker"));
    }

    fn rule_files(root: &Path) -> Vec<PathBuf> {
        let mut files = fs::read_dir(root)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "conf"))
            .collect::<Vec<_>>();
        files.sort();
        files
    }

    #[test]
    fn pinned_crs_action_inventory_when_configured() {
        let Some(root) = std::env::var_os("EDGION_WAF_CRS_RULES_DIR") else {
            return;
        };
        let files = rule_files(Path::new(&root));
        let mut directives_total = 0;
        let mut lists_total = 0;
        let mut entries_total = 0;
        let mut transformations = 0;
        let mut action_names = BTreeMap::<String, usize>::new();
        let mut transform_names = BTreeMap::<String, usize>::new();
        let mut single_quoted = 0;
        let mut unquoted = 0;
        let mut valueless = 0;
        let mut macro_values = 0;
        let mut macro_occurrences = 0;
        let mut escaped_single_values = 0;
        let mut escaped_single_occurrences = 0;
        let mut escaped_double_values = 0;
        let mut escaped_double_occurrences = 0;
        let mut empty_entries = 0;
        let mut max_entries = 0;
        let mut max_body = 0;
        let mut max_body_entry = String::new();
        let mut max_body_envelope = 0;
        let mut chains = 0;
        let mut charged_total = 0;
        let mut retained_total = 0;
        for path in &files {
            let bytes = fs::read(path).unwrap();
            let source_id = path
                .file_name()
                .expect("pinned CRS rule path must have a file name")
                .to_string_lossy();
            let source = scan_source(&source_id, &bytes, SourceLimits::default()).unwrap();
            let tokenized = tokenize_document(&source, TokenizerLimits::default()).unwrap();
            let directives = parse_directives(&tokenized, DirectiveLimits::default()).unwrap();
            directives_total += directives.len();
            let targets = parse_target_syntax(&directives, TargetSyntaxLimits::default()).unwrap();
            let operators = parse_operator_syntax(directives, targets, OperatorSyntaxLimits::default()).unwrap();
            let actions = parse_action_syntax(operators, ActionSyntaxLimits::default(), usize::MAX).unwrap();
            charged_total += actions.charged_bytes();
            retained_total += actions.retained_charged_bytes();
            lists_total += actions.list_count();
            for list in actions.lists() {
                max_entries = max_entries.max(list.entries().len());
                for entry in list.entries() {
                    entries_total += 1;
                    *action_names.entry(entry.name().text().to_owned()).or_default() += 1;
                    if entry.name().text().eq_ignore_ascii_case("chain") {
                        chains += 1;
                    }
                    if entry.full().text().is_empty() {
                        empty_entries += 1;
                    }
                    if entry.kind() == ActionEntryKind::Transformation {
                        transformations += 1;
                        if let Some(value) = entry.value() {
                            *transform_names.entry(value.body().text().to_owned()).or_default() += 1;
                        }
                    }
                    match entry.value() {
                        None => valueless += 1,
                        Some(value) => {
                            match value.spelling() {
                                ActionValueSpelling::SingleQuoted => single_quoted += 1,
                                ActionValueSpelling::Unquoted => unquoted += 1,
                            }
                            let body = value.body().text();
                            macro_values += usize::from(body.contains("%{"));
                            macro_occurrences += body.match_indices("%{").count();
                            escaped_single_values += usize::from(body.contains("\\'"));
                            escaped_single_occurrences += body.match_indices("\\'").count();
                            escaped_double_values += usize::from(body.contains("\\\""));
                            escaped_double_occurrences += body.match_indices("\\\"").count();
                            if body.len() > max_body {
                                max_body = body.len();
                                max_body_entry = entry.name().text().to_owned();
                                max_body_envelope = value.envelope().text().len();
                            }
                        }
                    }
                }
            }
        }
        eprintln!("action_names={action_names:?}");
        eprintln!("transform_names={transform_names:?}");
        eprintln!("max_body_entry={max_body_entry} envelope={max_body_envelope}");
        eprintln!(
            "macro_values={macro_values} macro_occurrences={macro_occurrences} escaped_single_values={escaped_single_values} escaped_single_occurrences={escaped_single_occurrences} escaped_double_values={escaped_double_values} escaped_double_occurrences={escaped_double_occurrences}"
        );
        eprintln!(
            "sizes mapping={} list={} entry={} lists_total={lists_total} charged_total={charged_total} retained_total={retained_total}",
            size_of::<ActionMapping>(),
            size_of::<ActionListRecord>(),
            size_of::<ActionEntry>()
        );
        assert_eq!(files.len(), 27);
        assert_eq!(directives_total, 786);
        assert_eq!(entries_total, 8_975);
        assert_eq!(max_entries, 29);
        assert_eq!(transformations, 832);
        assert_eq!(lists_total, 698);
        assert_eq!(
            action_names,
            BTreeMap::from([
                ("auditlog".to_owned(), 1),
                ("block".to_owned(), 341),
                ("capture".to_owned(), 275),
                ("chain".to_owned(), 73),
                ("ctl".to_owned(), 18),
                ("deny".to_owned(), 6),
                ("id".to_owned(), 627),
                ("initcol".to_owned(), 2),
                ("log".to_owned(), 8),
                ("logdata".to_owned(), 327),
                ("msg".to_owned(), 351),
                ("multiMatch".to_owned(), 8),
                ("noauditlog".to_owned(), 5),
                ("nolog".to_owned(), 278),
                ("pass".to_owned(), 280),
                ("phase".to_owned(), 627),
                ("setvar".to_owned(), 678),
                ("severity".to_owned(), 342),
                ("skip".to_owned(), 1),
                ("skipAfter".to_owned(), 206),
                ("status".to_owned(), 2),
                ("t".to_owned(), 832),
                ("tag".to_owned(), 3_060),
                ("ver".to_owned(), 627),
            ])
        );
        assert_eq!(
            transform_names,
            BTreeMap::from([
                ("base64Decode".to_owned(), 3),
                ("cmdLine".to_owned(), 13),
                ("compressWhitespace".to_owned(), 5),
                ("cssDecode".to_owned(), 23),
                ("escapeSeqDecode".to_owned(), 7),
                ("hexEncode".to_owned(), 2),
                ("htmlEntityDecode".to_owned(), 35),
                ("jsDecode".to_owned(), 37),
                ("length".to_owned(), 3),
                ("lowercase".to_owned(), 46),
                ("none".to_owned(), 427),
                ("normalizePath".to_owned(), 8),
                ("normalizePathWin".to_owned(), 4),
                ("removeCommentsChar".to_owned(), 1),
                ("removeNulls".to_owned(), 27),
                ("removeWhitespace".to_owned(), 11),
                ("replaceComments".to_owned(), 9),
                ("sha1".to_owned(), 2),
                ("urlDecodeUni".to_owned(), 137),
                ("utf8toUnicode".to_owned(), 32),
            ])
        );
        assert_eq!((single_quoted, unquoted, valueless), (5_385, 2_315, 1_275));
        assert_eq!((macro_values, macro_occurrences), (966, 1_485));
        assert_eq!((escaped_single_values, escaped_single_occurrences), (2, 4));
        assert_eq!((escaped_double_values, escaped_double_occurrences), (1, 2));
        assert_eq!((empty_entries, max_body, max_body_envelope, chains), (0, 876, 878, 73));
        assert_eq!(
            (
                size_of::<ActionMapping>(),
                size_of::<ActionListRecord>(),
                size_of::<ActionEntry>()
            ),
            (16, 104, 336)
        );
        assert_eq!(charged_total, 3_100_768);
        assert_eq!(retained_total, 5_355_977);
    }
}
