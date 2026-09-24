use std::mem::size_of;
use std::str;

use thiserror::Error;

/// Default maximum bytes in a source identifier.
pub const DEFAULT_MAX_SOURCE_ID_BYTES: usize = 4 * 1024;
/// Default maximum bytes in one SecLang source.
pub const DEFAULT_MAX_SOURCE_BYTES: usize = 16 * 1024 * 1024;
/// Default maximum bytes in one physical line, excluding its line ending.
pub const DEFAULT_MAX_PHYSICAL_LINE_BYTES: usize = 1024 * 1024;
/// Default maximum normalized bytes in one logical line.
pub const DEFAULT_MAX_LOGICAL_LINE_BYTES: usize = 4 * 1024 * 1024;
/// Default maximum physical lines in one source.
pub const DEFAULT_MAX_PHYSICAL_LINES: usize = 250_000;
/// Default maximum logical lines in one source.
pub const DEFAULT_MAX_LOGICAL_LINES: usize = 100_000;
/// Default maximum provenance segments in one source.
pub const DEFAULT_MAX_PROVENANCE_SEGMENTS: usize = 250_000;
/// Default maximum charged bytes owned by one scanned source document.
pub const DEFAULT_MAX_OWNED_BYTES: usize = 32 * 1024 * 1024;

/// Resource limits for scanning one SecLang source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceLimits {
    /// Maximum bytes in the caller-provided source identifier.
    pub max_source_id_bytes: usize,
    /// Maximum bytes in the input source.
    pub max_source_bytes: usize,
    /// Maximum bytes in one physical line, excluding LF or CRLF.
    pub max_physical_line_bytes: usize,
    /// Maximum normalized bytes in one logical line.
    pub max_logical_line_bytes: usize,
    /// Maximum physical lines in the source.
    pub max_physical_lines: usize,
    /// Maximum logical lines in the source.
    pub max_logical_lines: usize,
    /// Maximum logical-to-physical provenance segments.
    pub max_provenance_segments: usize,
    /// Maximum charged bytes owned by the returned document.
    pub max_owned_bytes: usize,
}

impl Default for SourceLimits {
    fn default() -> Self {
        Self {
            max_source_id_bytes: DEFAULT_MAX_SOURCE_ID_BYTES,
            max_source_bytes: DEFAULT_MAX_SOURCE_BYTES,
            max_physical_line_bytes: DEFAULT_MAX_PHYSICAL_LINE_BYTES,
            max_logical_line_bytes: DEFAULT_MAX_LOGICAL_LINE_BYTES,
            max_physical_lines: DEFAULT_MAX_PHYSICAL_LINES,
            max_logical_lines: DEFAULT_MAX_LOGICAL_LINES,
            max_provenance_segments: DEFAULT_MAX_PROVENANCE_SEGMENTS,
            max_owned_bytes: DEFAULT_MAX_OWNED_BYTES,
        }
    }
}

/// A position in the original source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourcePosition {
    /// One-based physical line number.
    pub line: usize,
    /// One-based byte column within the physical line.
    pub column: usize,
    /// Zero-based byte offset in the original source.
    pub byte_offset: usize,
}

/// A half-open range in the original source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceSpan {
    /// Inclusive start position.
    pub start: SourcePosition,
    /// Exclusive end position.
    pub end: SourcePosition,
}

/// A contiguous mapping from normalized logical bytes to original source bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProvenanceSegment {
    logical_start: usize,
    logical_end: usize,
    source_start: SourcePosition,
    source_end: SourcePosition,
}

impl ProvenanceSegment {
    /// Half-open normalized logical byte range covered by this segment.
    pub const fn logical_range(&self) -> (usize, usize) {
        (self.logical_start, self.logical_end)
    }

    /// Half-open original source span covered by this segment.
    pub const fn source_span(&self) -> SourceSpan {
        SourceSpan {
            start: self.source_start,
            end: self.source_end,
        }
    }
}

/// One normalized SecLang logical line.
#[derive(Debug, PartialEq, Eq)]
pub struct LogicalLine {
    text: String,
    span: SourceSpan,
    provenance: Vec<ProvenanceSegment>,
}

impl LogicalLine {
    /// Normalized logical-line text.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Half-open span from the first contribution to the last contribution.
    pub const fn span(&self) -> SourceSpan {
        self.span
    }

    /// Logical-to-physical provenance segments in source order.
    pub fn provenance(&self) -> &[ProvenanceSegment] {
        &self.provenance
    }

    /// Map a normalized logical byte offset back to the original source.
    ///
    /// At a continuation join, the boundary maps to the following non-empty
    /// segment. The one-past-end offset maps to the logical line's span end.
    pub fn position_at(&self, logical_offset: usize) -> Option<SourcePosition> {
        if logical_offset > self.text.len() {
            return None;
        }
        if logical_offset == self.text.len() {
            return Some(self.span.end);
        }
        let segment_index = self
            .provenance
            .partition_point(|segment| segment.logical_end <= logical_offset);
        self.provenance.get(segment_index).and_then(|segment| {
            (logical_offset >= segment.logical_start).then_some(SourcePosition {
                line: segment.source_start.line,
                column: segment.source_start.column + logical_offset - segment.logical_start,
                byte_offset: segment.source_start.byte_offset + logical_offset - segment.logical_start,
            })
        })
    }

    /// Map an exclusive normalized logical byte offset back to the original source.
    ///
    /// At a continuation join, the boundary maps to the preceding non-empty
    /// segment. Offset zero has no preceding byte and therefore uses the same
    /// right-biased mapping as [`Self::position_at`].
    pub fn position_at_end(&self, logical_offset: usize) -> Option<SourcePosition> {
        if logical_offset > self.text.len() {
            return None;
        }
        if logical_offset == 0 {
            return self.position_at(0);
        }
        if logical_offset == self.text.len() {
            return Some(self.span.end);
        }
        let segment_index = self
            .provenance
            .partition_point(|segment| segment.logical_start < logical_offset);
        segment_index.checked_sub(1).and_then(|index| {
            let segment = self.provenance.get(index)?;
            (logical_offset <= segment.logical_end).then_some(SourcePosition {
                line: segment.source_start.line,
                column: segment.source_start.column + logical_offset - segment.logical_start,
                byte_offset: segment.source_start.byte_offset + logical_offset - segment.logical_start,
            })
        })
    }
}

/// A bounded, normalized SecLang source document.
#[derive(Debug, PartialEq, Eq)]
pub struct SourceDocument {
    source_id: String,
    lines: Vec<LogicalLine>,
    physical_line_count: usize,
    provenance_segment_count: usize,
    charged_bytes: usize,
}

impl SourceDocument {
    /// Caller-provided source identity used by later diagnostics.
    pub fn source_id(&self) -> &str {
        &self.source_id
    }

    /// Normalized logical lines in source order.
    pub fn lines(&self) -> &[LogicalLine] {
        &self.lines
    }

    /// Number of physical lines observed in the source.
    pub const fn physical_line_count(&self) -> usize {
        self.physical_line_count
    }

    /// Number of retained provenance segments.
    pub const fn provenance_segment_count(&self) -> usize {
        self.provenance_segment_count
    }

    /// Bytes charged to the document's configured owned-memory budget.
    pub const fn charged_bytes(&self) -> usize {
        self.charged_bytes
    }
}

/// Error returned by the bounded SecLang source scanner.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum SourceError {
    /// The source identifier exceeds its configured byte limit.
    #[error("source identifier has {actual} bytes, exceeding the configured limit of {limit}")]
    SourceIdLimit { actual: usize, limit: usize },
    /// The source exceeds its configured byte limit.
    #[error("source has {actual} bytes, exceeding the configured limit of {limit} at {position:?}")]
    SourceByteLimit {
        actual: usize,
        limit: usize,
        position: SourcePosition,
    },
    /// The source is not valid UTF-8.
    #[error("SecLang source is not valid UTF-8 at {position:?}; use the UTF-8 compatibility profile")]
    InvalidUtf8 { position: SourcePosition },
    /// A bare carriage return was found.
    #[error("bare carriage return at {position:?}; use LF or CRLF line endings")]
    BareCarriageReturn { position: SourcePosition },
    /// The physical-line count exceeds its configured limit.
    #[error("physical line count exceeds the configured limit of {limit} at {position:?}")]
    PhysicalLineLimit { limit: usize, position: SourcePosition },
    /// One physical line exceeds its configured byte limit.
    #[error("physical line {line} has {actual} bytes, exceeding the configured limit of {limit} at {position:?}")]
    PhysicalLineByteLimit {
        line: usize,
        actual: usize,
        limit: usize,
        position: SourcePosition,
    },
    /// The logical-line count exceeds its configured limit.
    #[error("logical line count exceeds the configured limit of {limit} at {position:?}")]
    LogicalLineLimit { limit: usize, position: SourcePosition },
    /// One normalized logical line exceeds its configured byte limit.
    #[error("logical line has more than {limit} bytes at {position:?}")]
    LogicalLineByteLimit { limit: usize, position: SourcePosition },
    /// The provenance-segment count exceeds its configured limit.
    #[error("source requires more than {limit} provenance segments at {position:?}")]
    ProvenanceSegmentLimit { limit: usize, position: SourcePosition },
    /// The returned document would exceed its charged owned-byte limit.
    #[error("scanned source requires more than {limit} charged owned bytes at {position:?}")]
    OwnedByteLimit { limit: usize, position: SourcePosition },
    /// EOF was reached while a continuation was open.
    #[error("SecLang source ends with an unfinished continuation at {position:?}")]
    UnfinishedContinuation { position: SourcePosition },
    /// A bounded allocation failed.
    #[error("cannot allocate bounded SecLang {operation}; reduce the source or configured limits")]
    Allocation { operation: &'static str },
    /// The scanner reached an invalid internal state.
    #[error("SecLang source scanner cannot finalize its current logical line at {position:?}")]
    InvalidScannerState { position: SourcePosition },
}

#[derive(Debug)]
struct LogicalLineBuilder {
    text: String,
    provenance: Vec<ProvenanceSegment>,
    fallback_start: SourcePosition,
    end: SourcePosition,
}

impl LogicalLineBuilder {
    fn new(position: SourcePosition) -> Self {
        Self {
            text: String::new(),
            provenance: Vec::new(),
            fallback_start: position,
            end: position,
        }
    }

    fn finish(self) -> LogicalLine {
        LogicalLine {
            text: self.text,
            span: SourceSpan {
                start: self.fallback_start,
                end: self.end,
            },
            provenance: self.provenance,
        }
    }
}

/// Scan caller-owned SecLang bytes into bounded normalized logical lines.
pub fn scan_source(source_id: &str, source: &[u8], limits: SourceLimits) -> Result<SourceDocument, SourceError> {
    if source.len() > limits.max_source_bytes {
        return Err(SourceError::SourceByteLimit {
            actual: source.len(),
            limit: limits.max_source_bytes,
            position: position_for_offset(source, limits.max_source_bytes),
        });
    }
    if source_id.len() > limits.max_source_id_bytes {
        return Err(SourceError::SourceIdLimit {
            actual: source_id.len(),
            limit: limits.max_source_id_bytes,
        });
    }
    str::from_utf8(source).map_err(|error| SourceError::InvalidUtf8 {
        position: position_for_offset(source, error.valid_up_to()),
    })?;

    let origin = SourcePosition {
        line: 1,
        column: 1,
        byte_offset: 0,
    };
    let mut charged_bytes = 0;
    charge_owned(&mut charged_bytes, source_id.len(), limits.max_owned_bytes, origin)?;
    let mut owned_source_id = String::new();
    owned_source_id
        .try_reserve_exact(source_id.len())
        .map_err(|_| SourceError::Allocation {
            operation: "source identifier",
        })?;
    owned_source_id.push_str(source_id);

    let mut lines = Vec::new();
    let mut builder: Option<LogicalLineBuilder> = None;
    let mut cursor = 0;
    let mut physical_line_count = 0_usize;
    let mut logical_line_count = 0_usize;
    let mut provenance_segment_count = 0_usize;
    let mut continuation_position = None;

    while cursor < source.len() {
        let physical_start = cursor;
        let relative_lf = source[cursor..].iter().position(|byte| *byte == b'\n');
        let (raw_end, next_cursor, has_lf) = match relative_lf {
            Some(relative) => {
                let end = cursor + relative;
                (end, end + 1, true)
            }
            None => (source.len(), source.len(), false),
        };
        let content_end = if has_lf && raw_end > physical_start && source[raw_end - 1] == b'\r' {
            raw_end - 1
        } else {
            raw_end
        };

        let next_physical_count = physical_line_count
            .checked_add(1)
            .ok_or(SourceError::PhysicalLineLimit {
                limit: limits.max_physical_lines,
                position: position_in_line(physical_line_count.saturating_add(1), physical_start, physical_start),
            })?;
        if next_physical_count > limits.max_physical_lines {
            return Err(SourceError::PhysicalLineLimit {
                limit: limits.max_physical_lines,
                position: position_in_line(next_physical_count, physical_start, physical_start),
            });
        }
        physical_line_count = next_physical_count;

        if let Some(relative_cr) = source[physical_start..content_end]
            .iter()
            .position(|byte| *byte == b'\r')
        {
            let offset = physical_start + relative_cr;
            return Err(SourceError::BareCarriageReturn {
                position: position_in_line(physical_line_count, physical_start, offset),
            });
        }
        if !has_lf && raw_end > physical_start && source[raw_end - 1] == b'\r' {
            return Err(SourceError::BareCarriageReturn {
                position: position_in_line(physical_line_count, physical_start, raw_end - 1),
            });
        }

        let physical_len = content_end - physical_start;
        if physical_len > limits.max_physical_line_bytes {
            return Err(SourceError::PhysicalLineByteLimit {
                line: physical_line_count,
                actual: physical_len,
                limit: limits.max_physical_line_bytes,
                position: position_in_line(
                    physical_line_count,
                    physical_start,
                    physical_start + limits.max_physical_line_bytes,
                ),
            });
        }

        let (trimmed_start, trimmed_end) = trim_sp_htab(source, physical_start, content_end);
        if trimmed_start == trimmed_end || source[trimmed_start] == b'#' {
            cursor = next_cursor;
            continue;
        }

        let continued = source[trimmed_end - 1] == b'\\';
        let piece_end = if continued { trimmed_end - 1 } else { trimmed_end };
        let contribution_start = position_in_line(physical_line_count, physical_start, trimmed_start);
        let contribution_end = position_in_line(physical_line_count, physical_start, piece_end);
        let current = builder.get_or_insert_with(|| LogicalLineBuilder::new(contribution_start));
        let logical_start = current.text.len();
        let piece_len = piece_end - trimmed_start;
        let logical_end = logical_start
            .checked_add(piece_len)
            .ok_or(SourceError::LogicalLineByteLimit {
                limit: limits.max_logical_line_bytes,
                position: contribution_start,
            })?;
        if logical_end > limits.max_logical_line_bytes {
            let allowed_from_piece = limits.max_logical_line_bytes.saturating_sub(logical_start);
            return Err(SourceError::LogicalLineByteLimit {
                limit: limits.max_logical_line_bytes,
                position: position_in_line(physical_line_count, physical_start, trimmed_start + allowed_from_piece),
            });
        }

        let next_segment_count =
            provenance_segment_count
                .checked_add(1)
                .ok_or(SourceError::ProvenanceSegmentLimit {
                    limit: limits.max_provenance_segments,
                    position: contribution_start,
                })?;
        if next_segment_count > limits.max_provenance_segments {
            return Err(SourceError::ProvenanceSegmentLimit {
                limit: limits.max_provenance_segments,
                position: contribution_start,
            });
        }
        charge_owned(
            &mut charged_bytes,
            piece_len,
            limits.max_owned_bytes,
            contribution_start,
        )?;
        charge_owned(
            &mut charged_bytes,
            size_of::<ProvenanceSegment>(),
            limits.max_owned_bytes,
            contribution_start,
        )?;
        current
            .text
            .try_reserve(piece_len)
            .map_err(|_| SourceError::Allocation {
                operation: "logical line text",
            })?;
        current.provenance.try_reserve(1).map_err(|_| SourceError::Allocation {
            operation: "source provenance",
        })?;
        current
            .text
            .push_str(
                str::from_utf8(&source[trimmed_start..piece_end]).map_err(|_| SourceError::InvalidUtf8 {
                    position: contribution_start,
                })?,
            );
        current.provenance.push(ProvenanceSegment {
            logical_start,
            logical_end,
            source_start: contribution_start,
            source_end: contribution_end,
        });
        current.end = contribution_end;
        provenance_segment_count = next_segment_count;

        if continued {
            continuation_position = Some(position_in_line(physical_line_count, physical_start, trimmed_end - 1));
        } else {
            continuation_position = None;
            let next_logical_count = logical_line_count.checked_add(1).ok_or(SourceError::LogicalLineLimit {
                limit: limits.max_logical_lines,
                position: contribution_start,
            })?;
            if next_logical_count > limits.max_logical_lines {
                return Err(SourceError::LogicalLineLimit {
                    limit: limits.max_logical_lines,
                    position: contribution_start,
                });
            }
            charge_owned(
                &mut charged_bytes,
                size_of::<LogicalLine>(),
                limits.max_owned_bytes,
                contribution_start,
            )?;
            lines.try_reserve(1).map_err(|_| SourceError::Allocation {
                operation: "logical line records",
            })?;
            let completed = builder.take().ok_or(SourceError::InvalidScannerState {
                position: contribution_start,
            })?;
            lines.push(completed.finish());
            logical_line_count = next_logical_count;
        }

        cursor = next_cursor;
    }

    if continuation_position.is_some() {
        return Err(SourceError::UnfinishedContinuation {
            position: position_for_offset(source, source.len()),
        });
    }

    Ok(SourceDocument {
        source_id: owned_source_id,
        lines,
        physical_line_count,
        provenance_segment_count,
        charged_bytes,
    })
}

fn trim_sp_htab(source: &[u8], mut start: usize, mut end: usize) -> (usize, usize) {
    while start < end && matches!(source[start], b' ' | b'\t') {
        start += 1;
    }
    while end > start && matches!(source[end - 1], b' ' | b'\t') {
        end -= 1;
    }
    (start, end)
}

fn charge_owned(charged: &mut usize, amount: usize, limit: usize, position: SourcePosition) -> Result<(), SourceError> {
    let next = charged
        .checked_add(amount)
        .ok_or(SourceError::OwnedByteLimit { limit, position })?;
    if next > limit {
        return Err(SourceError::OwnedByteLimit { limit, position });
    }
    *charged = next;
    Ok(())
}

fn position_for_offset(source: &[u8], requested_offset: usize) -> SourcePosition {
    let offset = requested_offset.min(source.len());
    let mut line = 1_usize;
    let mut column = 1_usize;
    let mut cursor = 0_usize;
    while cursor < offset {
        match source[cursor] {
            b'\r' if cursor + 1 < source.len() && source[cursor + 1] == b'\n' => {
                if cursor + 1 >= offset {
                    column += 1;
                    cursor += 1;
                } else {
                    line += 1;
                    column = 1;
                    cursor += 2;
                }
            }
            b'\n' => {
                line += 1;
                column = 1;
                cursor += 1;
            }
            _ => {
                column += 1;
                cursor += 1;
            }
        }
    }
    SourcePosition {
        line,
        column,
        byte_offset: offset,
    }
}

fn position_in_line(line: usize, physical_start: usize, byte_offset: usize) -> SourcePosition {
    SourcePosition {
        line,
        column: byte_offset.saturating_sub(physical_start).saturating_add(1),
        byte_offset,
    }
}

#[cfg(test)]
#[path = "../../tests/unit/seclang/source_test.rs"]
mod tests;
