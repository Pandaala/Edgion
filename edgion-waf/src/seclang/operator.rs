use std::mem::size_of;
use std::ops::Range;
use std::ptr;

use thiserror::Error;

use super::target::TargetSyntaxDocument;
use super::{Directive, DirectiveDocument, LogicalLine, SourceDocument, SourcePosition, SourceSpan, Token};

const DEFAULT_MAX_RULE_OPERATORS: usize = 100_000;
const DEFAULT_MAX_ESCAPES_PER_OPERATOR: usize = 65_536;
const DEFAULT_MAX_OPERATOR_ESCAPES: usize = 262_144;
const DEFAULT_MAX_DECODED_OPERATOR_BYTES: usize = 16 * 1024 * 1024;
const DEFAULT_MAX_OPERATOR_OWNED_BYTES: usize = 32 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperatorSyntaxLimits {
    pub max_rule_operators: usize,
    pub max_escapes_per_operator: usize,
    pub max_document_escapes: usize,
    pub max_decoded_bytes: usize,
    pub max_owned_bytes: usize,
}

impl Default for OperatorSyntaxLimits {
    fn default() -> Self {
        Self {
            max_rule_operators: DEFAULT_MAX_RULE_OPERATORS,
            max_escapes_per_operator: DEFAULT_MAX_ESCAPES_PER_OPERATOR,
            max_document_escapes: DEFAULT_MAX_OPERATOR_ESCAPES,
            max_decoded_bytes: DEFAULT_MAX_DECODED_OPERATOR_BYTES,
            max_owned_bytes: DEFAULT_MAX_OPERATOR_OWNED_BYTES,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperatorSpelling {
    ImplicitRx,
    Explicit,
}

#[derive(Debug, PartialEq, Eq)]
enum OperatorMapping {
    None,
    Rule(usize),
}

#[derive(Debug, PartialEq, Eq)]
struct OperatorRecord<'source> {
    logical_line_index: usize,
    content: &'source str,
    content_logical_start: usize,
    content_span: SourceSpan,
    spelling: OperatorSpelling,
    negated: bool,
    explicit_spelling_start: usize,
    explicit_spelling_end: usize,
    name_start: usize,
    name_end: usize,
    argument_start: usize,
    argument_end: usize,
    explicit_spelling_span: SourceSpan,
    name_span: SourceSpan,
    argument_span: SourceSpan,
    escape_start: usize,
    escape_count: usize,
    decoded_len: usize,
}

#[derive(Debug)]
pub(super) struct OperatorSyntaxDocument<'source> {
    directives: DirectiveDocument<'source>,
    targets: TargetSyntaxDocument<'source>,
    mappings: Vec<OperatorMapping>,
    operators: Vec<OperatorRecord<'source>>,
    escape_offsets: Vec<usize>,
    charged_bytes: usize,
    decoded_bytes: usize,
}

impl<'source> OperatorSyntaxDocument<'source> {
    pub(super) fn source(&self) -> &'source SourceDocument {
        self.directives.source()
    }

    pub(super) fn targets(&self) -> &TargetSyntaxDocument<'source> {
        &self.targets
    }

    pub(super) fn charged_bytes(&self) -> usize {
        self.charged_bytes
    }

    pub(super) fn decoded_bytes(&self) -> usize {
        self.decoded_bytes
    }

    pub(super) fn operator_count(&self) -> usize {
        self.operators.len()
    }

    pub(super) fn directives(&self) -> &DirectiveDocument<'source> {
        &self.directives
    }

    pub(super) fn mapping_count(&self) -> usize {
        self.mappings.len()
    }

    pub(super) fn retained_charged_bytes(&self) -> Option<usize> {
        self.source()
            .charged_bytes()
            .checked_add(self.directives.charged_bytes())?
            .checked_add(self.targets.charged_bytes())?
            .checked_add(self.charged_bytes)
    }

    pub(super) fn get(&self, index: usize) -> Option<OperatorMappingView<'_, 'source>> {
        match *self.mappings.get(index)? {
            OperatorMapping::None => Some(OperatorMappingView::None),
            OperatorMapping::Rule(record) => Some(OperatorMappingView::Rule(operator_view(
                self.operators
                    .get(record)
                    .expect("private operator mapping must remain valid"),
                &self.escape_offsets,
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy)]
#[allow(clippy::large_enum_variant)] // Short-lived views stay allocation-free and borrow the flat arena.
pub enum OperatorMappingView<'view, 'source> {
    None,
    Rule(OperatorView<'view, 'source>),
}

#[derive(Debug, Clone, Copy)]
pub struct OperatorView<'view, 'source> {
    logical_line_index: usize,
    content: DecodedTextView<'view, 'source>,
    content_logical_start: usize,
    content_span: SourceSpan,
    spelling: OperatorSpelling,
    negated: bool,
    explicit_spelling: DecodedTextView<'view, 'source>,
    name: DecodedTextView<'view, 'source>,
    argument: DecodedTextView<'view, 'source>,
    explicit_spelling_span: SourceSpan,
    name_span: SourceSpan,
    argument_span: SourceSpan,
}

#[derive(Debug, Clone, Copy)]
pub enum OperatorNameView<'view, 'source> {
    ImplicitRx,
    Explicit(DecodedTextView<'view, 'source>),
}

impl<'view, 'source> OperatorView<'view, 'source> {
    pub fn logical_line_index(&self) -> usize {
        self.logical_line_index
    }

    pub fn content(&self) -> DecodedTextView<'view, 'source> {
        self.content
    }

    pub fn content_logical_range(&self) -> Range<usize> {
        self.content_logical_start..self.content_logical_start + self.content.raw().len()
    }

    pub fn content_span(&self) -> SourceSpan {
        self.content_span
    }

    pub fn spelling(&self) -> OperatorSpelling {
        self.spelling
    }

    pub fn negated(&self) -> bool {
        self.negated
    }

    pub fn explicit_spelling(&self) -> Option<DecodedTextView<'view, 'source>> {
        matches!(self.spelling, OperatorSpelling::Explicit).then_some(self.explicit_spelling)
    }

    pub fn explicit_spelling_logical_range(&self) -> Option<Range<usize>> {
        self.explicit_spelling().map(|value| self.raw_range(value))
    }

    pub fn explicit_spelling_span(&self) -> Option<SourceSpan> {
        self.explicit_spelling().map(|_| self.explicit_spelling_span)
    }

    pub fn name(&self) -> OperatorNameView<'view, 'source> {
        match self.spelling {
            OperatorSpelling::ImplicitRx => OperatorNameView::ImplicitRx,
            OperatorSpelling::Explicit => OperatorNameView::Explicit(self.name),
        }
    }

    pub fn name_logical_range(&self) -> Option<Range<usize>> {
        matches!(self.spelling, OperatorSpelling::Explicit).then(|| self.raw_range(self.name))
    }

    pub fn name_span(&self) -> Option<SourceSpan> {
        matches!(self.spelling, OperatorSpelling::Explicit).then_some(self.name_span)
    }

    pub fn argument(&self) -> DecodedTextView<'view, 'source> {
        self.argument
    }

    pub fn argument_logical_range(&self) -> Range<usize> {
        self.raw_range(self.argument)
    }

    pub fn argument_span(&self) -> SourceSpan {
        self.argument_span
    }

    pub(super) fn argument_has_removable_escapes(&self) -> bool {
        !self.argument.escapes.is_empty()
    }

    pub(super) fn map_argument_decoded_nonempty_range(&self, range: Range<usize>) -> Option<Range<usize>> {
        if range.start >= range.end || range.end > self.argument.decoded_len() {
            return None;
        }
        let start = self.argument.raw_boundary_for_decoded_offset(range.start)?;
        let end = self.argument.raw_boundary_for_decoded_offset(range.end)?;
        if !self.argument.raw.is_char_boundary(start) || !self.argument.raw.is_char_boundary(end) {
            return None;
        }
        let logical_start = self.argument_logical_range().start.checked_add(start)?;
        Some(logical_start..self.argument_logical_range().start.checked_add(end)?)
    }

    fn raw_range(&self, value: DecodedTextView<'_, '_>) -> Range<usize> {
        let start = self.content_logical_start + value.raw_start_in_content;
        start..start + value.raw.len()
    }
}

fn operator_view<'view, 'source>(
    record: &'view OperatorRecord<'source>,
    offsets: &'view [usize],
) -> OperatorView<'view, 'source> {
    let escapes = checked_offsets(offsets, record.escape_start, record.escape_count);
    OperatorView {
        logical_line_index: record.logical_line_index,
        content: decoded_subview(record.content, escapes, 0, record.content.len()),
        content_logical_start: record.content_logical_start,
        content_span: record.content_span,
        spelling: record.spelling,
        negated: record.negated,
        explicit_spelling: decoded_subview(
            record.content,
            escapes,
            record.explicit_spelling_start,
            record.explicit_spelling_end,
        ),
        name: decoded_subview(record.content, escapes, record.name_start, record.name_end),
        argument: decoded_subview(record.content, escapes, record.argument_start, record.argument_end),
        explicit_spelling_span: record.explicit_spelling_span,
        name_span: record.name_span,
        argument_span: record.argument_span,
    }
}

fn checked_offsets(values: &[usize], start: usize, count: usize) -> &[usize] {
    let end = start
        .checked_add(count)
        .expect("private operator escape count must fit after checked parser arithmetic");
    values
        .get(start..end)
        .expect("private operator escape range must remain valid in an immutable document")
}

#[derive(Debug, Clone, Copy)]
pub struct DecodedTextView<'view, 'source> {
    raw: &'source str,
    escapes: &'view [usize],
    raw_start_in_content: usize,
    decoded_len: usize,
}

impl<'view, 'source> DecodedTextView<'view, 'source> {
    pub fn raw(&self) -> &'source str {
        self.raw
    }

    pub fn decoded_len(&self) -> usize {
        self.decoded_len
    }

    pub fn as_borrowed(&self) -> Option<&'source str> {
        self.escapes.is_empty().then_some(self.raw)
    }

    pub fn chunks(&self) -> DecodedChunks<'view, 'source> {
        DecodedChunks {
            raw: self.raw,
            escapes: self.escapes,
            raw_start_in_content: self.raw_start_in_content,
            cursor: 0,
            escape_index: 0,
        }
    }

    pub fn bytes(&self) -> DecodedBytes<'view, 'source> {
        DecodedBytes {
            raw: self.raw.as_bytes(),
            escapes: self.escapes,
            raw_start_in_content: self.raw_start_in_content,
            cursor: 0,
            escape_index: 0,
        }
    }

    fn raw_boundary_for_decoded_offset(&self, decoded_offset: usize) -> Option<usize> {
        self.raw_boundary_for_decoded_offset_with_work(decoded_offset)
            .map(|(boundary, _)| boundary)
    }

    fn raw_boundary_for_decoded_offset_with_work(&self, decoded_offset: usize) -> Option<(usize, usize)> {
        if decoded_offset > self.decoded_len {
            return None;
        }
        if self.escapes.is_empty() {
            return Some((decoded_offset, 0));
        }
        let mut left = 0usize;
        let mut right = self.escapes.len();
        let mut comparisons = 0usize;
        while left < right {
            comparisons += 1;
            let middle = left + (right - left) / 2;
            let raw_escape = self.escapes[middle].checked_sub(self.raw_start_in_content)?;
            let decoded_escape = raw_escape.checked_sub(middle)?;
            if decoded_escape < decoded_offset {
                left = middle + 1;
            } else {
                right = middle;
            }
        }
        let raw = decoded_offset.checked_add(left)?;
        (raw <= self.raw.len()).then_some((raw, comparisons))
    }
}

fn decoded_subview<'view, 'source>(
    content: &'source str,
    escapes: &'view [usize],
    start: usize,
    end: usize,
) -> DecodedTextView<'view, 'source> {
    let first = escapes.partition_point(|offset| *offset < start);
    let last = escapes.partition_point(|offset| *offset < end);
    let selected = &escapes[first..last];
    DecodedTextView {
        raw: content
            .get(start..end)
            .expect("private operator subrange must remain on UTF-8 boundaries"),
        escapes: selected,
        raw_start_in_content: start,
        decoded_len: end - start - selected.len(),
    }
}

pub struct DecodedChunks<'view, 'source> {
    raw: &'source str,
    escapes: &'view [usize],
    raw_start_in_content: usize,
    cursor: usize,
    escape_index: usize,
}

impl<'source> Iterator for DecodedChunks<'_, 'source> {
    type Item = &'source str;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if self.escape_index < self.escapes.len() {
                let escape = self.escapes[self.escape_index] - self.raw_start_in_content;
                let start = self.cursor;
                self.cursor = escape + 1;
                self.escape_index += 1;
                if start < escape {
                    return self.raw.get(start..escape);
                }
            } else if self.cursor < self.raw.len() {
                let start = self.cursor;
                self.cursor = self.raw.len();
                return self.raw.get(start..);
            } else {
                return None;
            }
        }
    }
}

#[derive(Clone)]
pub struct DecodedBytes<'view, 'source> {
    raw: &'source [u8],
    escapes: &'view [usize],
    raw_start_in_content: usize,
    cursor: usize,
    escape_index: usize,
}

impl Iterator for DecodedBytes<'_, '_> {
    type Item = u8;

    fn next(&mut self) -> Option<Self::Item> {
        while self.cursor < self.raw.len() {
            let content_offset = self.raw_start_in_content + self.cursor;
            if self.escape_index < self.escapes.len() && self.escapes[self.escape_index] == content_offset {
                self.cursor += 1;
                self.escape_index += 1;
                continue;
            }
            let byte = self.raw[self.cursor];
            self.cursor += 1;
            return Some(byte);
        }
        None
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum OperatorSyntaxError {
    #[error("operator and target documents retain different source identities")]
    SourceMismatch,
    #[error("operator input has {directives} directives but {target_records} target records")]
    RecordCountMismatch { directives: usize, target_records: usize },
    #[error("operator syntax mapping metadata requires more than {limit} owned bytes")]
    MappingOwnedByteLimit { limit: usize },
    #[error("rule operator count exceeds the configured limit of {limit} at {position:?}")]
    OperatorLimit { limit: usize, position: SourcePosition },
    #[error("rule operator has {actual} removable quote escapes, exceeding the per-operator limit of {limit} at {position:?}")]
    OperatorEscapeLimit {
        actual: usize,
        limit: usize,
        position: SourcePosition,
    },
    #[error("operator document has more than {limit} removable quote escapes at {position:?}")]
    DocumentEscapeLimit { limit: usize, position: SourcePosition },
    #[error("operator document has more than {limit} decoded bytes at {position:?}")]
    DecodedByteLimit { limit: usize, position: SourcePosition },
    #[error("operator syntax requires more than {limit} charged owned bytes at {position:?}")]
    OwnedByteLimit { limit: usize, position: SourcePosition },
    #[error("operator token is unavailable on logical line {logical_line}")]
    MissingOperator { logical_line: usize },
    #[error("cannot map operator syntax on logical line {logical_line} at byte offset {logical_offset}")]
    PositionMapping { logical_line: usize, logical_offset: usize },
    #[error("cannot allocate bounded SecLang {operation}; reduce the source or configured limits")]
    Allocation { operation: &'static str },
}

pub(super) fn parse_operator_syntax<'source>(
    directives: DirectiveDocument<'source>,
    targets: TargetSyntaxDocument<'source>,
    limits: OperatorSyntaxLimits,
) -> Result<OperatorSyntaxDocument<'source>, OperatorSyntaxError> {
    if !ptr::eq(directives.source(), targets.source()) {
        return Err(OperatorSyntaxError::SourceMismatch);
    }
    if directives.len() != targets.record_count() {
        return Err(OperatorSyntaxError::RecordCountMismatch {
            directives: directives.len(),
            target_records: targets.record_count(),
        });
    }

    let mapping_bytes = size_of::<OperatorMapping>().checked_mul(directives.len()).ok_or(
        OperatorSyntaxError::MappingOwnedByteLimit {
            limit: limits.max_owned_bytes,
        },
    )?;
    if mapping_bytes > limits.max_owned_bytes {
        return Err(OperatorSyntaxError::MappingOwnedByteLimit {
            limit: limits.max_owned_bytes,
        });
    }
    let mut mappings = Vec::new();
    mappings
        .try_reserve_exact(directives.len())
        .map_err(|_| OperatorSyntaxError::Allocation {
            operation: "operator mapping records",
        })?;
    let mut document = OperatorSyntaxDocument {
        directives,
        targets,
        mappings,
        operators: Vec::new(),
        escape_offsets: Vec::new(),
        charged_bytes: mapping_bytes,
        decoded_bytes: 0,
    };

    for directive in document.directives.iter() {
        let Directive::SecRule { operator, .. } = directive.directive() else {
            document.mappings.push(OperatorMapping::None);
            continue;
        };
        if document.operators.len() >= limits.max_rule_operators {
            return Err(OperatorSyntaxError::OperatorLimit {
                limit: limits.max_rule_operators,
                position: operator.content_span().start,
            });
        }
        let line_index = directive.logical_line_index();
        let line = document
            .source()
            .lines()
            .get(line_index)
            .ok_or(OperatorSyntaxError::PositionMapping {
                logical_line: line_index + 1,
                logical_offset: operator.content_logical_range().start,
            })?;
        parse_one_operator(
            operator,
            line,
            line_index,
            limits,
            &mut document.operators,
            &mut document.escape_offsets,
            &mut document.charged_bytes,
            &mut document.decoded_bytes,
        )?;
        document
            .mappings
            .push(OperatorMapping::Rule(document.operators.len() - 1));
    }
    Ok(document)
}

#[allow(clippy::too_many_arguments)] // Keeps independently bounded flat arenas and source mapping explicit.
fn parse_one_operator<'source>(
    token: Token<'source>,
    line: &LogicalLine,
    logical_line_index: usize,
    limits: OperatorSyntaxLimits,
    operators: &mut Vec<OperatorRecord<'source>>,
    escape_offsets: &mut Vec<usize>,
    charged_bytes: &mut usize,
    decoded_bytes: &mut usize,
) -> Result<(), OperatorSyntaxError> {
    let content = token.value();
    let escape_count = count_quote_escapes(content.as_bytes());
    if escape_count > limits.max_escapes_per_operator {
        return Err(OperatorSyntaxError::OperatorEscapeLimit {
            actual: escape_count,
            limit: limits.max_escapes_per_operator,
            position: token.content_span().start,
        });
    }
    let next_escape_count =
        escape_offsets
            .len()
            .checked_add(escape_count)
            .ok_or(OperatorSyntaxError::DocumentEscapeLimit {
                limit: limits.max_document_escapes,
                position: token.content_span().start,
            })?;
    if next_escape_count > limits.max_document_escapes {
        return Err(OperatorSyntaxError::DocumentEscapeLimit {
            limit: limits.max_document_escapes,
            position: token.content_span().start,
        });
    }
    let decoded_len = content
        .len()
        .checked_sub(escape_count)
        .ok_or(OperatorSyntaxError::DecodedByteLimit {
            limit: limits.max_decoded_bytes,
            position: token.content_span().start,
        })?;
    let next_decoded = decoded_bytes
        .checked_add(decoded_len)
        .ok_or(OperatorSyntaxError::DecodedByteLimit {
            limit: limits.max_decoded_bytes,
            position: token.content_span().start,
        })?;
    if next_decoded > limits.max_decoded_bytes {
        return Err(OperatorSyntaxError::DecodedByteLimit {
            limit: limits.max_decoded_bytes,
            position: token.content_span().start,
        });
    }

    let classification = classify(content);
    let explicit_spelling_span = map_raw_span(
        line,
        logical_line_index,
        token,
        classification.explicit_spelling.clone(),
    )?;
    let name_span = map_raw_span(line, logical_line_index, token, classification.name.clone())?;
    let argument_span = map_raw_span(line, logical_line_index, token, classification.argument.clone())?;
    let escape_bytes = size_of::<usize>()
        .checked_mul(escape_count)
        .ok_or(OperatorSyntaxError::OwnedByteLimit {
            limit: limits.max_owned_bytes,
            position: token.content_span().start,
        })?;
    let owned_add =
        size_of::<OperatorRecord<'source>>()
            .checked_add(escape_bytes)
            .ok_or(OperatorSyntaxError::OwnedByteLimit {
                limit: limits.max_owned_bytes,
                position: token.content_span().start,
            })?;
    charge_owned(
        charged_bytes,
        owned_add,
        limits.max_owned_bytes,
        token.content_span().start,
    )?;
    escape_offsets
        .try_reserve_exact(escape_count)
        .map_err(|_| OperatorSyntaxError::Allocation {
            operation: "operator escape offsets",
        })?;
    operators.try_reserve(1).map_err(|_| OperatorSyntaxError::Allocation {
        operation: "operator records",
    })?;

    let escape_start = escape_offsets.len();
    fill_quote_escapes(content.as_bytes(), escape_offsets);
    operators.push(OperatorRecord {
        logical_line_index,
        content,
        content_logical_start: token.content_logical_range().start,
        content_span: token.content_span(),
        spelling: classification.spelling,
        negated: classification.negated,
        explicit_spelling_start: classification.explicit_spelling.start,
        explicit_spelling_end: classification.explicit_spelling.end,
        name_start: classification.name.start,
        name_end: classification.name.end,
        argument_start: classification.argument.start,
        argument_end: classification.argument.end,
        explicit_spelling_span,
        name_span,
        argument_span,
        escape_start,
        escape_count,
        decoded_len,
    });
    *decoded_bytes = next_decoded;
    Ok(())
}

fn map_raw_span(
    line: &LogicalLine,
    line_index: usize,
    token: Token<'_>,
    range: Range<usize>,
) -> Result<SourceSpan, OperatorSyntaxError> {
    let base = token.content_logical_range().start;
    let start = base
        .checked_add(range.start)
        .ok_or(OperatorSyntaxError::PositionMapping {
            logical_line: line_index + 1,
            logical_offset: base,
        })?;
    let end = base
        .checked_add(range.end)
        .ok_or(OperatorSyntaxError::PositionMapping {
            logical_line: line_index + 1,
            logical_offset: base,
        })?;
    let start_position = line.position_at(start).ok_or(OperatorSyntaxError::PositionMapping {
        logical_line: line_index + 1,
        logical_offset: start,
    })?;
    let end_position = if start == end {
        start_position
    } else {
        line.position_at_end(end).ok_or(OperatorSyntaxError::PositionMapping {
            logical_line: line_index + 1,
            logical_offset: end,
        })?
    };
    Ok(SourceSpan {
        start: start_position,
        end: end_position,
    })
}

fn count_quote_escapes(bytes: &[u8]) -> usize {
    bytes.windows(2).filter(|pair| pair == b"\\\"").count()
}

fn fill_quote_escapes(bytes: &[u8], offsets: &mut Vec<usize>) {
    for (offset, pair) in bytes.windows(2).enumerate() {
        if pair == b"\\\"" {
            offsets.push(offset);
        }
    }
}

struct Classification {
    spelling: OperatorSpelling,
    negated: bool,
    explicit_spelling: Range<usize>,
    name: Range<usize>,
    argument: Range<usize>,
}

fn classify(content: &str) -> Classification {
    let bytes = content.as_bytes();
    if bytes.is_empty() {
        return implicit(false, 0..0);
    }
    if !matches!(bytes[0], b'@' | b'!') {
        return implicit(false, trim_sp_htab(bytes, 0..bytes.len()));
    }
    let trimmed = trim_sp_htab(bytes, 0..bytes.len());
    if bytes[0] == b'!' {
        if trimmed.len() == 1 {
            return implicit(true, trimmed.end..trimmed.end);
        }
        if bytes.get(1) != Some(&b'@') {
            return implicit(true, trim_sp_htab(bytes, 1..bytes.len()));
        }
        if trimmed.len() == 2 {
            return Classification {
                spelling: OperatorSpelling::Explicit,
                negated: false,
                explicit_spelling: trimmed.clone(),
                name: trimmed.clone(),
                argument: trimmed.end..trimmed.end,
            };
        }
        return explicit(bytes, trimmed.clone(), trimmed.start + 2, true);
    }
    explicit(bytes, trimmed.clone(), trimmed.start + 1, false)
}

fn implicit(negated: bool, argument: Range<usize>) -> Classification {
    Classification {
        spelling: OperatorSpelling::ImplicitRx,
        negated,
        explicit_spelling: argument.start..argument.start,
        name: argument.start..argument.start,
        argument,
    }
}

fn explicit(bytes: &[u8], spelling: Range<usize>, body_start: usize, negated: bool) -> Classification {
    let split = bytes[body_start..spelling.end]
        .iter()
        .position(|byte| *byte == b' ')
        .map(|relative| body_start + relative);
    let (name, argument) = match split {
        Some(space) => (
            trim_sp_htab(bytes, body_start..space),
            trim_sp_htab(bytes, space + 1..spelling.end),
        ),
        None => (
            trim_sp_htab(bytes, body_start..spelling.end),
            spelling.end..spelling.end,
        ),
    };
    Classification {
        spelling: OperatorSpelling::Explicit,
        negated,
        explicit_spelling: spelling,
        name,
        argument,
    }
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
) -> Result<(), OperatorSyntaxError> {
    let next = charged
        .checked_add(amount)
        .ok_or(OperatorSyntaxError::OwnedByteLimit { limit, position })?;
    if next > limit {
        return Err(OperatorSyntaxError::OwnedByteLimit { limit, position });
    }
    *charged = next;
    Ok(())
}

#[cfg(test)]
#[path = "../../tests/unit/seclang/operator_test.rs"]
mod tests;

#[cfg(test)]
#[path = "../../tests/support/seclang/operator.rs"]
mod test_support;
#[cfg(test)]
pub(super) use test_support::operator_record_size_for_test;
