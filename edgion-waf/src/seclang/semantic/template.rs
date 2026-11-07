use super::*;

#[derive(Debug, Clone)]
pub(super) struct MacroScan {
    pub(super) start: usize,
    pub(super) end: usize,
    body: Range<usize>,
    source: Range<usize>,
    key: Option<Range<usize>>,
    kind: MacroSource,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct TemplateShape {
    pub(super) segments: usize,
    pub(super) expansions: usize,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct MacroParseError {
    pub(super) kind: MacroErrorKind,
    offset: usize,
}

const MACRO_SOURCES: [(&[u8], MacroSource, bool); 7] = [
    (b"TX", MacroSource::Tx, true),
    (b"ARGS", MacroSource::Args, true),
    (b"REQUEST_HEADERS", MacroSource::RequestHeaders, true),
    (b"MATCHED_VAR", MacroSource::MatchedVar, false),
    (b"MATCHED_VAR_NAME", MacroSource::MatchedVarName, false),
    (b"REMOTE_ADDR", MacroSource::RemoteAddr, false),
    (b"REQUEST_LINE", MacroSource::RequestLine, false),
];

pub(super) fn walk_template<I, F>(bytes: I, mut on_macro: F) -> Result<TemplateShape, MacroParseError>
where
    I: IntoIterator<Item = u8>,
    F: FnMut(MacroScan),
{
    let mut bytes = bytes.into_iter().peekable();
    let mut offset = 0usize;
    let mut literal_start = 0usize;
    let mut segments = 0usize;
    let mut expansions = 0usize;
    while let Some(byte) = bytes.next() {
        let current = offset;
        offset += 1;
        if byte != b'%' || bytes.peek() != Some(&b'{') {
            continue;
        }
        bytes.next();
        offset += 1;
        let body_start = offset;
        let mut source_end = None;
        let mut key_start = None;
        let mut candidates = [true; 7];
        let mut source_offset = 0usize;
        let close = loop {
            let Some(part) = bytes.next() else {
                return Err(MacroParseError {
                    kind: MacroErrorKind::Unclosed,
                    offset: current,
                });
            };
            let part_offset = offset;
            offset += 1;
            if part == b'}' {
                break part_offset;
            }
            if part == b'{' || (part == b'%' && bytes.peek() == Some(&b'{')) {
                return Err(MacroParseError {
                    kind: MacroErrorKind::Nested,
                    offset: current,
                });
            }
            if !part.is_ascii_alphanumeric() && !matches!(part, b'_' | b'.' | b'[' | b']' | b'-') {
                return Err(MacroParseError {
                    kind: MacroErrorKind::InvalidCharacter,
                    offset: current,
                });
            }
            if source_end.is_none() && part == b'.' {
                source_end = Some(part_offset);
                key_start = Some(offset);
            } else if source_end.is_none() {
                for (candidate, (name, _, _)) in candidates.iter_mut().zip(MACRO_SOURCES) {
                    *candidate &= name
                        .get(source_offset)
                        .is_some_and(|expected| expected.eq_ignore_ascii_case(&part));
                }
                source_offset += 1;
            }
        };
        if close == body_start {
            return Err(MacroParseError {
                kind: MacroErrorKind::EmptyBody,
                offset: current,
            });
        }
        let source_end = source_end.unwrap_or(close);
        let Some((_, kind, requires_key)) = MACRO_SOURCES.into_iter().enumerate().find_map(|(index, value)| {
            (candidates[index] && value.0.len() == source_end - body_start).then_some(value)
        }) else {
            return Err(MacroParseError {
                kind: MacroErrorKind::UnknownSource,
                offset: body_start,
            });
        };
        let key = key_start.map(|start| start..close);
        if requires_key && key.as_ref().is_none_or(Range::is_empty) {
            return Err(MacroParseError {
                kind: MacroErrorKind::MissingKey,
                offset: key_start.unwrap_or(source_end),
            });
        }
        if !requires_key && key.is_some() {
            return Err(MacroParseError {
                kind: MacroErrorKind::ForbiddenKey,
                offset: key_start.unwrap_or(source_end),
            });
        }
        if current > literal_start {
            segments += 1;
        }
        expansions += 1;
        segments += 1;
        on_macro(MacroScan {
            start: current,
            end: offset,
            body: body_start..close,
            source: body_start..source_end,
            key,
            kind,
        });
        literal_start = offset;
    }
    if offset > literal_start {
        segments += 1;
    }
    Ok(TemplateShape { segments, expansions })
}

pub(super) fn operator_macro_capable(kind: OperatorKind) -> bool {
    matches!(
        kind,
        OperatorKind::BeginsWith
            | OperatorKind::Contains
            | OperatorKind::EndsWith
            | OperatorKind::Eq
            | OperatorKind::Ge
            | OperatorKind::Gt
            | OperatorKind::Lt
            | OperatorKind::Streq
            | OperatorKind::Within
    )
}

pub(super) struct TemplateBuild<'a, 'source> {
    pub(super) templates: &'a mut Vec<TemplateRecord<'source>>,
    pub(super) segments: &'a mut Vec<TemplateSegmentRecord>,
    pub(super) expansions: &'a mut Vec<ExpansionRecord<'source>>,
    pub(super) decoded: &'a mut Vec<u8>,
    pub(super) requirements: &'a mut Vec<RequirementRecord>,
    pub(super) semantic_charge: &'a mut usize,
    pub(super) scanned: &'a mut usize,
    pub(super) inherited: usize,
    pub(super) limits: SemanticLimits,
}

impl<'source> TemplateBuild<'_, 'source> {
    pub(super) fn check_operand_scan(
        &self,
        operand_bytes: usize,
        position: SourcePosition,
    ) -> Result<(), SemanticError> {
        if operand_bytes > self.limits.operands.max_operand_bytes {
            return Err(SemanticError::OperandByteLimit {
                limit: self.limits.operands.max_operand_bytes,
                position,
            });
        }
        let scanned = self
            .scanned
            .checked_add(operand_bytes)
            .ok_or(SemanticError::ScannedOperandByteLimit {
                limit: self.limits.operands.max_scanned_bytes,
                position,
            })?;
        if scanned > self.limits.operands.max_scanned_bytes {
            return Err(SemanticError::ScannedOperandByteLimit {
                limit: self.limits.operands.max_scanned_bytes,
                position,
            });
        }
        Ok(())
    }

    fn prepare(
        &mut self,
        shape: TemplateShape,
        operand_bytes: usize,
        copied_bytes: usize,
        location: SemanticLocation,
        position: SourcePosition,
    ) -> Result<(), SemanticError> {
        let operand_limits = self.limits.operands;
        if operand_bytes > operand_limits.max_operand_bytes {
            return Err(SemanticError::OperandByteLimit {
                limit: operand_limits.max_operand_bytes,
                position,
            });
        }
        let scanned = self
            .scanned
            .checked_add(operand_bytes)
            .ok_or(SemanticError::ScannedOperandByteLimit {
                limit: operand_limits.max_scanned_bytes,
                position,
            })?;
        if scanned > operand_limits.max_scanned_bytes {
            return Err(SemanticError::ScannedOperandByteLimit {
                limit: operand_limits.max_scanned_bytes,
                position,
            });
        }
        if self
            .templates
            .len()
            .checked_add(1)
            .is_none_or(|count| count > operand_limits.max_templates)
        {
            return Err(SemanticError::TemplateLimit {
                limit: operand_limits.max_templates,
                position,
            });
        }
        if shape.expansions > operand_limits.max_expansions_per_template {
            return Err(SemanticError::TemplateExpansionLimit {
                limit: operand_limits.max_expansions_per_template,
                position,
            });
        }
        if self
            .segments
            .len()
            .checked_add(shape.segments)
            .is_none_or(|count| count > operand_limits.max_segments)
        {
            return Err(SemanticError::TemplateSegmentLimit {
                limit: operand_limits.max_segments,
                position,
            });
        }
        if self
            .expansions
            .len()
            .checked_add(shape.expansions)
            .is_none_or(|count| count > operand_limits.max_expansions)
        {
            return Err(SemanticError::ExpansionLimit {
                limit: operand_limits.max_expansions,
                position,
            });
        }
        if self
            .decoded
            .len()
            .checked_add(copied_bytes)
            .is_none_or(|count| count > operand_limits.max_decoded_bytes)
        {
            return Err(SemanticError::DecodedOperandByteLimit {
                limit: operand_limits.max_decoded_bytes,
                position,
            });
        }
        let dynamic = size_of::<TemplateRecord<'_>>()
            .checked_add(size_of::<TemplateSegmentRecord>().checked_mul(shape.segments).ok_or(
                SemanticError::OwnedByteLimit {
                    limit: self.limits.max_owned_bytes,
                    position,
                },
            )?)
            .and_then(|value| value.checked_add(size_of::<ExpansionRecord<'_>>().checked_mul(shape.expansions)?))
            .and_then(|value| value.checked_add(copied_bytes))
            .and_then(|value| {
                value.checked_add(if shape.expansions > 0 {
                    size_of::<RequirementRecord>()
                } else {
                    0
                })
            })
            .ok_or(SemanticError::OwnedByteLimit {
                limit: self.limits.max_owned_bytes,
                position,
            })?;
        let charge = self
            .semantic_charge
            .checked_add(dynamic)
            .ok_or(SemanticError::OwnedByteLimit {
                limit: self.limits.max_owned_bytes,
                position,
            })?;
        if charge > self.limits.max_owned_bytes {
            return Err(SemanticError::OwnedByteLimit {
                limit: self.limits.max_owned_bytes,
                position,
            });
        }
        if self
            .inherited
            .checked_add(charge)
            .is_none_or(|retained| retained > self.limits.max_retained_bytes)
        {
            return Err(SemanticError::RetainedByteLimit {
                limit: self.limits.max_retained_bytes,
            });
        }
        if shape.expansions > 0 && self.requirements.len() == self.requirements.capacity() {
            return Err(SemanticError::Allocation {
                operation: "preflight compile requirements",
            });
        }
        self.templates.try_reserve(1).map_err(|_| SemanticError::Allocation {
            operation: "operand templates",
        })?;
        self.segments
            .try_reserve(shape.segments)
            .map_err(|_| SemanticError::Allocation {
                operation: "template segments",
            })?;
        self.expansions
            .try_reserve(shape.expansions)
            .map_err(|_| SemanticError::Allocation {
                operation: "macro expansions",
            })?;
        self.decoded
            .try_reserve(copied_bytes)
            .map_err(|_| SemanticError::Allocation {
                operation: "decoded operands",
            })?;
        *self.semantic_charge = charge;
        *self.scanned = scanned;
        if shape.expansions > 0 {
            self.requirements.push(RequirementRecord {
                location,
                requirement: CompileRequirement::RuntimeMacro,
            });
        }
        Ok(())
    }
}

fn mapped_text<'source>(
    line: &'source LogicalLine,
    logical: Range<usize>,
) -> Result<SemanticText<'source>, SemanticError> {
    let text = line
        .text()
        .get(logical.clone())
        .ok_or(SemanticError::OperandPositionMapping {
            logical_line: line.span().start.line,
            logical_offset: logical.start,
        })?;
    let start = line
        .position_at(logical.start)
        .ok_or(SemanticError::OperandPositionMapping {
            logical_line: line.span().start.line,
            logical_offset: logical.start,
        })?;
    let end = line
        .position_at_end(logical.end)
        .ok_or(SemanticError::OperandPositionMapping {
            logical_line: line.span().start.line,
            logical_offset: logical.end,
        })?;
    Ok(SemanticText {
        text,
        logical_start: logical.start,
        logical_end: logical.end,
        span: SourceSpan { start, end },
    })
}

pub(super) fn action_mapped_text<'source>(
    operand: SemanticText<'source>,
    line: &'source LogicalLine,
    range: Range<usize>,
) -> Result<SemanticText<'source>, SemanticError> {
    let logical = operand
        .logical_start
        .checked_add(range.start)
        .and_then(|start| operand.logical_start.checked_add(range.end).map(|end| start..end))
        .ok_or(SemanticError::OperandPositionMapping {
            logical_line: line.span().start.line,
            logical_offset: range.start,
        })?;
    mapped_text(line, logical)
}

pub(super) fn operator_mapped_text<'source>(
    operator: OperatorView<'_, 'source>,
    line: &'source LogicalLine,
    range: Range<usize>,
) -> Result<SemanticText<'source>, SemanticError> {
    let logical =
        operator
            .map_argument_decoded_nonempty_range(range.clone())
            .ok_or(SemanticError::OperandPositionMapping {
                logical_line: operator.logical_line_index() + 1,
                logical_offset: range.start,
            })?;
    mapped_text(line, logical)
}

pub(super) fn macro_error_position_action(
    error: MacroParseError,
    operand: SemanticText<'_>,
    line: &LogicalLine,
) -> SourcePosition {
    if error.offset < operand.text().len() {
        action_mapped_text(operand, line, error.offset..error.offset + 1)
            .map_or(operand.span().start, |text| text.span().start)
    } else {
        operand.span().end
    }
}

pub(super) fn macro_error_position_operator(
    error: MacroParseError,
    operator: OperatorView<'_, '_>,
    line: &LogicalLine,
) -> SourcePosition {
    if error.offset < operator.argument().decoded_len() {
        operator_mapped_text(operator, line, error.offset..error.offset + 1)
            .map_or(operator.argument_span().start, |text| text.span().start)
    } else {
        operator.argument_span().end
    }
}

pub(super) fn validate_template_mapping<I, F>(bytes: I, decoded_len: usize, mut map: F) -> Result<(), SemanticError>
where
    I: IntoIterator<Item = u8>,
    F: FnMut(Range<usize>) -> Result<SourceSpan, SemanticError>,
{
    let mut failure = None;
    let mut literal = 0usize;
    walk_template(bytes, |macro_| {
        if failure.is_some() {
            return;
        }
        for range in [
            (macro_.start > literal).then_some(literal..macro_.start),
            Some(macro_.start..macro_.end),
            Some(macro_.body.clone()),
            Some(macro_.source.clone()),
            macro_.key.clone(),
        ]
        .into_iter()
        .flatten()
        {
            if let Err(error) = map(range) {
                failure = Some(error);
                break;
            }
        }
        literal = macro_.end;
    })
    .expect("template was validated by the allocation-free count pass");
    if failure.is_none() && literal < decoded_len {
        if let Err(error) = map(literal..decoded_len) {
            failure = Some(error);
        }
    }
    failure.map_or(Ok(()), Err)
}

pub(super) fn push_template_records<'source, I, F>(
    bytes: I,
    decoded_len: usize,
    storage: OperandStorage<'source>,
    templates: &mut Vec<TemplateRecord<'source>>,
    segments: &mut Vec<TemplateSegmentRecord>,
    expansions: &mut Vec<ExpansionRecord<'source>>,
    mut map: F,
) -> usize
where
    I: IntoIterator<Item = u8>,
    F: FnMut(Range<usize>) -> SemanticText<'source>,
{
    let template = templates.len();
    let segment_start = segments.len();
    let mut literal = 0usize;
    let shape = walk_template(bytes, |macro_| {
        if macro_.start > literal {
            let text = map(literal..macro_.start);
            segments.push(TemplateSegmentRecord {
                kind: TemplateSegmentRecordKind::Literal,
                start: literal,
                end: macro_.start,
                span: text.span(),
            });
        }
        let expansion = expansions.len();
        expansions.push(ExpansionRecord {
            source: macro_.kind,
            body: map(macro_.body.clone()),
            source_spelling: map(macro_.source.clone()),
            key: macro_.key.clone().map(&mut map),
        });
        let text = map(macro_.start..macro_.end);
        segments.push(TemplateSegmentRecord {
            kind: TemplateSegmentRecordKind::Expansion(expansion),
            start: macro_.start,
            end: macro_.end,
            span: text.span(),
        });
        literal = macro_.end;
    })
    .expect("template was validated before bounded arena mutation");
    if literal < decoded_len {
        let text = map(literal..decoded_len);
        segments.push(TemplateSegmentRecord {
            kind: TemplateSegmentRecordKind::Literal,
            start: literal,
            end: decoded_len,
            span: text.span(),
        });
    }
    templates.push(TemplateRecord {
        storage,
        segment_start,
        segment_count: shape.segments,
        expansion_count: shape.expansions,
    });
    template
}

pub(super) fn attach_operator_template<'source>(
    build: &mut TemplateBuild<'_, 'source>,
    operator: OperatorView<'_, 'source>,
    line: &'source LogicalLine,
    location: SemanticLocation,
) -> Result<usize, SemanticError> {
    let argument = operator.argument();
    let decoded_len = argument.decoded_len();
    build.check_operand_scan(decoded_len, operator.argument_span().start)?;
    let shape = walk_template(argument.bytes(), |_| {}).map_err(|error| SemanticError::MacroSyntax {
        kind: error.kind,
        position: macro_error_position_operator(error, operator, line),
    })?;
    validate_template_mapping(argument.bytes(), decoded_len, |range| {
        operator_mapped_text(operator, line, range).map(|text| text.span())
    })?;
    let copied = if operator.argument_has_removable_escapes() {
        decoded_len
    } else {
        0
    };
    build.prepare(shape, decoded_len, copied, location, operator.argument_span().start)?;
    let storage = if copied == 0 {
        let range = operator.argument_logical_range();
        OperandStorage::Borrowed(SemanticText {
            text: argument.raw(),
            logical_start: range.start,
            logical_end: range.end,
            span: operator.argument_span(),
        })
    } else {
        let start = build.decoded.len();
        build.decoded.extend(argument.bytes());
        OperandStorage::Decoded {
            start,
            len: decoded_len,
            span: operator.argument_span(),
        }
    };
    Ok(push_template_records(
        argument.bytes(),
        decoded_len,
        storage,
        build.templates,
        build.segments,
        build.expansions,
        |range| operator_mapped_text(operator, line, range).expect("operator subranges were validated before mutation"),
    ))
}
