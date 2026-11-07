use super::*;

#[derive(Debug, Clone)]
struct SlicePlan {
    range: Range<usize>,
    expansions: usize,
}

#[derive(Debug, Clone)]
struct SetVarPlan {
    operation: SetVarOperation,
    key: SlicePlan,
    value: Option<SlicePlan>,
    constant: Option<i64>,
}

#[derive(Debug, Clone)]
struct InitColPlan {
    collection: InitColCollection,
    key: SlicePlan,
}

#[derive(Debug, Clone)]
enum CtlPlan<'source> {
    AuditEngine(AuditEngineMode),
    ForceRequestBodyVariable(OnOff),
    RequestBodyProcessor(RequestBodyProcessor),
    RuleRemoveByTag(SemanticText<'source>),
    RuleRemoveTargetByTag {
        tag: SemanticText<'source>,
        target_range: Range<usize>,
    },
}

enum ActionPlan<'source> {
    NoArgument,
    Local(LocalActionValue),
    Transformation(TransformationKind),
    Template,
    Literal(SemanticText<'source>),
    SetVar(SetVarPlan),
    InitCol(InitColPlan),
    Ctl(CtlPlan<'source>),
}

pub(super) struct ActionOperandBuild<'a, 'source> {
    pub(super) templates: &'a mut Vec<TemplateRecord<'source>>,
    pub(super) segments: &'a mut Vec<TemplateSegmentRecord>,
    pub(super) expansions: &'a mut Vec<ExpansionRecord<'source>>,
    pub(super) operands: &'a mut Vec<ActionOperandRecord<'source>>,
    pub(super) setvars: &'a mut Vec<SetVarRecord>,
    pub(super) initcols: &'a mut Vec<InitColRecord>,
    pub(super) ctls: &'a mut Vec<CtlRecord<'source>>,
    pub(super) ctl_targets: &'a mut Vec<CtlTargetRecord<'source>>,
    pub(super) structured_count: &'a mut usize,
    pub(super) requirements: &'a mut Vec<RequirementRecord>,
    pub(super) unavailable: &'a mut Vec<CapabilityRecord>,
    pub(super) semantic_charge: &'a mut usize,
    pub(super) scanned: &'a mut usize,
    pub(super) inherited: usize,
    pub(super) limits: SemanticLimits,
}

fn action_position(operand: SemanticText<'_>, line: &LogicalLine, offset: usize) -> SourcePosition {
    if offset < operand.text().len() {
        action_mapped_text(operand, line, offset..offset + 1).map_or(operand.span().start, |text| text.span().start)
    } else {
        operand.span().end
    }
}

fn plan_slice<I>(bytes: I, range: Range<usize>) -> Result<SlicePlan, ()>
where
    I: IntoIterator<Item = u8>,
{
    let mut expansions = 0usize;
    let mut crossing = false;
    walk_template(bytes, |macro_| {
        let overlaps = macro_.start < range.end && macro_.end > range.start;
        if overlaps {
            if macro_.start < range.start || macro_.end > range.end {
                crossing = true;
            } else {
                expansions += 1;
            }
        }
    })
    .expect("action template was validated before slice planning");
    if crossing {
        Err(())
    } else {
        Ok(SlicePlan { range, expansions })
    }
}

fn parse_setvar_plan(
    operand: SemanticText<'_>,
    line: &LogicalLine,
) -> Result<(SetVarPlan, TemplateShape), SemanticError> {
    let bytes = operand.text().as_bytes();
    let (operation, key_range, value_range) = if bytes.first() == Some(&b'!') {
        if bytes.iter().position(|byte| *byte == b'=').is_some() {
            return Err(SemanticError::SetVarSyntax {
                kind: SetVarErrorKind::Operation,
                position: operand.span().start,
            });
        }
        if bytes.len() < 5 || !bytes[1..3].eq_ignore_ascii_case(b"TX") || bytes[3] != b'.' {
            return Err(SemanticError::SetVarSyntax {
                kind: SetVarErrorKind::Collection,
                position: action_position(operand, line, 1.min(bytes.len())),
            });
        }
        (SetVarOperation::Delete, 4..bytes.len(), None)
    } else {
        if bytes.len() < 4 || !bytes[..2].eq_ignore_ascii_case(b"TX") || bytes[2] != b'.' {
            return Err(SemanticError::SetVarSyntax {
                kind: SetVarErrorKind::Collection,
                position: operand.span().start,
            });
        }
        let equals = bytes
            .iter()
            .position(|byte| *byte == b'=')
            .ok_or(SemanticError::SetVarSyntax {
                kind: SetVarErrorKind::Envelope,
                position: operand.span().end,
            })?;
        let value_start = equals + 1;
        let (operation, value_start) = match bytes.get(value_start) {
            Some(b'+') => (SetVarOperation::Add, value_start + 1),
            Some(b'-') => (SetVarOperation::Subtract, value_start + 1),
            Some(_) => (SetVarOperation::Assign, value_start),
            None => {
                return Err(SemanticError::SetVarSyntax {
                    kind: SetVarErrorKind::Value,
                    position: operand.span().end,
                })
            }
        };
        (operation, 3..equals, Some(value_start..bytes.len()))
    };
    if key_range.is_empty() {
        return Err(SemanticError::SetVarSyntax {
            kind: SetVarErrorKind::Key,
            position: action_position(operand, line, key_range.start),
        });
    }
    if value_range.as_ref().is_some_and(Range::is_empty) {
        return Err(SemanticError::SetVarSyntax {
            kind: SetVarErrorKind::Value,
            position: operand.span().end,
        });
    }
    let shape = walk_template(operand.text().bytes(), |_| {}).map_err(|error| SemanticError::MacroSyntax {
        kind: error.kind,
        position: macro_error_position_action(error, operand, line),
    })?;
    for (offset, byte) in bytes[key_range.clone()].iter().copied().enumerate() {
        if matches!(byte, b' ' | b'\t' | b'=') || byte.is_ascii_control() || byte == 0x7f {
            return Err(SemanticError::SetVarSyntax {
                kind: SetVarErrorKind::Key,
                position: action_position(operand, line, key_range.start + offset),
            });
        }
    }
    let key =
        plan_slice(operand.text().bytes(), key_range.clone()).map_err(|()| SemanticError::TemplateSliceInvariant {
            position: action_position(operand, line, key_range.start),
        })?;
    let value = value_range
        .clone()
        .map(|range| {
            plan_slice(operand.text().bytes(), range.clone()).map_err(|()| SemanticError::TemplateSliceInvariant {
                position: action_position(operand, line, range.start),
            })
        })
        .transpose()?;
    let constant = if matches!(operation, SetVarOperation::Add | SetVarOperation::Subtract)
        && value.as_ref().is_some_and(|slice| slice.expansions == 0)
    {
        let range = value_range.expect("arithmetic operation has a value range");
        Some(
            strict_i64(bytes[range.clone()].iter().copied()).map_err(|(kind, offset)| {
                SemanticError::IntegerOperand {
                    kind,
                    position: action_position(
                        operand,
                        line,
                        range.start
                            + if kind == IntegerOperandErrorKind::Overflow {
                                0
                            } else {
                                offset
                            },
                    ),
                }
            })?,
        )
    } else {
        None
    };
    Ok((
        SetVarPlan {
            operation,
            key,
            value,
            constant,
        },
        shape,
    ))
}

fn parse_initcol_plan(
    operand: SemanticText<'_>,
    line: &LogicalLine,
) -> Result<(InitColPlan, TemplateShape), SemanticError> {
    let bytes = operand.text().as_bytes();
    let equals = bytes
        .iter()
        .position(|byte| *byte == b'=')
        .ok_or(SemanticError::InitColSyntax {
            kind: InitColErrorKind::Envelope,
            position: operand.span().end,
        })?;
    if equals == 0 {
        return Err(SemanticError::InitColSyntax {
            kind: InitColErrorKind::Collection,
            position: operand.span().start,
        });
    }
    let collection = if bytes[..equals].eq_ignore_ascii_case(b"global") {
        InitColCollection::Global
    } else if bytes[..equals].eq_ignore_ascii_case(b"ip") {
        InitColCollection::Ip
    } else {
        return Err(SemanticError::InitColSyntax {
            kind: InitColErrorKind::Collection,
            position: operand.span().start,
        });
    };
    let key_range = equals + 1..bytes.len();
    if key_range.is_empty() {
        return Err(SemanticError::InitColSyntax {
            kind: InitColErrorKind::Key,
            position: operand.span().end,
        });
    }
    let shape = walk_template(operand.text().bytes(), |_| {}).map_err(|error| SemanticError::MacroSyntax {
        kind: error.kind,
        position: macro_error_position_action(error, operand, line),
    })?;
    let key =
        plan_slice(operand.text().bytes(), key_range.clone()).map_err(|()| SemanticError::TemplateSliceInvariant {
            position: action_position(operand, line, key_range.start),
        })?;
    Ok((InitColPlan { collection, key }, shape))
}

fn parse_on_off(bytes: &[u8]) -> Option<OnOff> {
    if bytes.eq_ignore_ascii_case(b"On") {
        Some(OnOff::On)
    } else if bytes.eq_ignore_ascii_case(b"Off") {
        Some(OnOff::Off)
    } else {
        None
    }
}

fn parse_ctl_plan<'source>(
    operand: SemanticText<'source>,
    line: &'source LogicalLine,
    line_index: usize,
) -> Result<CtlPlan<'source>, SemanticError> {
    let bytes = operand.text().as_bytes();
    let equals = bytes
        .iter()
        .position(|byte| *byte == b'=')
        .ok_or(SemanticError::CtlSyntax {
            kind: CtlErrorKind::Envelope,
            position: operand.span().end,
        })?;
    if equals == 0 || equals + 1 == bytes.len() {
        return Err(SemanticError::CtlSyntax {
            kind: CtlErrorKind::Envelope,
            position: action_position(operand, line, equals),
        });
    }
    let rhs = equals + 1..bytes.len();
    if matches!(bytes[equals - 1], b' ' | b'\t') || matches!(bytes[rhs.start], b' ' | b'\t') {
        return Err(SemanticError::CtlSyntax {
            kind: CtlErrorKind::Delimiter,
            position: action_position(operand, line, equals),
        });
    }
    if matches!(bytes[rhs.end - 1], b' ' | b'\t') {
        return Err(SemanticError::CtlSyntax {
            kind: CtlErrorKind::Value,
            position: action_position(operand, line, rhs.end - 1),
        });
    }
    let value = &bytes[rhs.clone()];
    match &bytes[..equals] {
        b"auditEngine" => {
            let mode = if value.eq_ignore_ascii_case(b"On") {
                AuditEngineMode::On
            } else if value.eq_ignore_ascii_case(b"Off") {
                AuditEngineMode::Off
            } else if value.eq_ignore_ascii_case(b"RelevantOnly") {
                AuditEngineMode::RelevantOnly
            } else {
                return Err(SemanticError::CtlSyntax {
                    kind: CtlErrorKind::Value,
                    position: action_position(operand, line, rhs.start),
                });
            };
            Ok(CtlPlan::AuditEngine(mode))
        }
        b"forceRequestBodyVariable" => {
            parse_on_off(value)
                .map(CtlPlan::ForceRequestBodyVariable)
                .ok_or(SemanticError::CtlSyntax {
                    kind: CtlErrorKind::Value,
                    position: action_position(operand, line, rhs.start),
                })
        }
        b"requestBodyProcessor" => {
            let processor = if value.eq_ignore_ascii_case(b"URLENCODED") {
                RequestBodyProcessor::UrlEncoded
            } else if value.eq_ignore_ascii_case(b"MULTIPART") {
                RequestBodyProcessor::Multipart
            } else if value.eq_ignore_ascii_case(b"JSON") {
                RequestBodyProcessor::Json
            } else if value.eq_ignore_ascii_case(b"XML") {
                RequestBodyProcessor::Xml
            } else {
                return Err(SemanticError::CtlSyntax {
                    kind: CtlErrorKind::Value,
                    position: action_position(operand, line, rhs.start),
                });
            };
            Ok(CtlPlan::RequestBodyProcessor(processor))
        }
        b"ruleRemoveByTag" => {
            if let Some(offset) = value.iter().position(|byte| *byte == b';') {
                return Err(SemanticError::CtlSyntax {
                    kind: CtlErrorKind::Delimiter,
                    position: action_position(operand, line, rhs.start + offset),
                });
            }
            Ok(CtlPlan::RuleRemoveByTag(action_mapped_text(operand, line, rhs)?))
        }
        b"ruleRemoveTargetByTag" => {
            let separator = value
                .iter()
                .position(|byte| *byte == b';')
                .ok_or(SemanticError::CtlSyntax {
                    kind: CtlErrorKind::Delimiter,
                    position: operand.span().end,
                })?;
            if value[separator + 1..].contains(&b';') {
                return Err(SemanticError::CtlSyntax {
                    kind: CtlErrorKind::Delimiter,
                    position: action_position(operand, line, rhs.start + separator + 1),
                });
            }
            let tag_range = rhs.start..rhs.start + separator;
            let target_range = tag_range.end + 1..rhs.end;
            if tag_range.is_empty() || target_range.is_empty() {
                return Err(SemanticError::CtlSyntax {
                    kind: CtlErrorKind::Delimiter,
                    position: action_position(operand, line, tag_range.end),
                });
            }
            if matches!(bytes[tag_range.end - 1], b' ' | b'\t') || matches!(bytes[target_range.start], b' ' | b'\t') {
                return Err(SemanticError::CtlSyntax {
                    kind: CtlErrorKind::Delimiter,
                    position: action_position(operand, line, tag_range.end),
                });
            }
            let tag = action_mapped_text(operand, line, tag_range)?;
            let target_text = action_mapped_text(operand, line, target_range.clone())?;
            let target = parse_single_target(target_text.text(), target_text.logical_range().start, line, line_index)
                .map_err(SemanticError::CtlTarget)?;
            if CollectionKind::lookup_ascii_insensitive(target.collection().text().as_bytes()).is_none() {
                return Err(SemanticError::CtlSyntax {
                    kind: CtlErrorKind::Collection,
                    position: target.collection().span().start,
                });
            }
            Ok(CtlPlan::RuleRemoveTargetByTag { tag, target_range })
        }
        _ => Err(SemanticError::CtlSyntax {
            kind: CtlErrorKind::Command,
            position: operand.span().start,
        }),
    }
}

fn commit_template_slice<'source>(
    plan: SlicePlan,
    template: usize,
    operand: SemanticText<'source>,
    line: &'source LogicalLine,
    templates: &[TemplateRecord<'source>],
    segments: &[TemplateSegmentRecord],
) -> TemplateSliceRecord {
    let record = templates[template];
    let all = &segments[record.segment_start..record.segment_start + record.segment_count];
    let first = all
        .iter()
        .position(|segment| segment.end > plan.range.start)
        .expect("nonempty template slice must overlap a validated segment");
    let last = all
        .iter()
        .rposition(|segment| segment.start < plan.range.end)
        .expect("nonempty template slice must overlap a validated segment");
    let span = action_mapped_text(operand, line, plan.range.clone())
        .expect("action slice mapping was validated before candidate commit")
        .span();
    TemplateSliceRecord {
        template,
        start: plan.range.start,
        end: plan.range.end,
        segment_start: record.segment_start + first,
        segment_count: last - first + 1,
        expansion_count: plan.expansions,
        span,
    }
}

pub(super) fn classify_action_operand<'source>(
    build: &mut ActionOperandBuild<'_, 'source>,
    envelope: ActionEnvelopeKind,
    operand: Option<SemanticText<'source>>,
    name: SemanticText<'source>,
    line: &'source LogicalLine,
    line_index: usize,
    location: SemanticLocation,
) -> Result<usize, SemanticError> {
    let operand_bytes = operand.map_or(0, |value| value.text().len());
    if operand_bytes > build.limits.operands.max_operand_bytes {
        return Err(SemanticError::OperandByteLimit {
            limit: build.limits.operands.max_operand_bytes,
            position: operand.map_or(name.span().end, |value| value.span().start),
        });
    }
    let scanned = build
        .scanned
        .checked_add(operand_bytes)
        .ok_or(SemanticError::ScannedOperandByteLimit {
            limit: build.limits.operands.max_scanned_bytes,
            position: operand.map_or(name.span().end, |value| value.span().start),
        })?;
    if scanned > build.limits.operands.max_scanned_bytes {
        return Err(SemanticError::ScannedOperandByteLimit {
            limit: build.limits.operands.max_scanned_bytes,
            position: operand.map_or(name.span().end, |value| value.span().start),
        });
    }

    let mut template_shape = None;
    let plan = match envelope {
        ActionEnvelopeKind::Transformation => {
            let value = required_operand(operand, name)?;
            let transformation = TransformationKind::lookup_ascii_insensitive(value.text().as_bytes()).ok_or(
                SemanticError::UnknownTransformation {
                    position: value.span().start,
                },
            )?;
            ActionPlan::Transformation(transformation)
        }
        ActionEnvelopeKind::Action(action) => match action {
            ActionKind::AuditLog
            | ActionKind::Block
            | ActionKind::Capture
            | ActionKind::Chain
            | ActionKind::Deny
            | ActionKind::Log
            | ActionKind::MultiMatch
            | ActionKind::NoAuditLog
            | ActionKind::NoLog
            | ActionKind::Pass => {
                forbidden_operand(operand, name)?;
                ActionPlan::NoArgument
            }
            ActionKind::Id => ActionPlan::Local(parse_id(required_operand(operand, name)?)?),
            ActionKind::Phase => ActionPlan::Local(parse_phase(required_operand(operand, name)?)?),
            ActionKind::Severity => ActionPlan::Local(parse_severity(required_operand(operand, name)?)?),
            ActionKind::Status => ActionPlan::Local(parse_status(required_operand(operand, name)?)?),
            ActionKind::Skip => ActionPlan::Local(parse_skip(required_operand(operand, name)?)?),
            ActionKind::Msg | ActionKind::LogData => {
                let value = required_operand(operand, name)?;
                let shape =
                    walk_template(value.text().bytes(), |_| {}).map_err(|error| SemanticError::MacroSyntax {
                        kind: error.kind,
                        position: macro_error_position_action(error, value, line),
                    })?;
                validate_template_mapping(value.text().bytes(), value.text().len(), |range| {
                    action_mapped_text(value, line, range).map(|text| text.span())
                })?;
                template_shape = Some(shape);
                ActionPlan::Template
            }
            ActionKind::SetVar => {
                let value = required_operand(operand, name)?;
                let (setvar, shape) = parse_setvar_plan(value, line)?;
                validate_template_mapping(value.text().bytes(), value.text().len(), |range| {
                    action_mapped_text(value, line, range).map(|text| text.span())
                })?;
                if setvar.key.range.len() > build.limits.operands.max_setvar_key_bytes {
                    return Err(SemanticError::SetVarKeyByteLimit {
                        limit: build.limits.operands.max_setvar_key_bytes,
                        position: action_position(value, line, setvar.key.range.start),
                    });
                }
                template_shape = Some(shape);
                ActionPlan::SetVar(setvar)
            }
            ActionKind::InitCol => {
                let value = required_operand(operand, name)?;
                let (initcol, shape) = parse_initcol_plan(value, line)?;
                validate_template_mapping(value.text().bytes(), value.text().len(), |range| {
                    action_mapped_text(value, line, range).map(|text| text.span())
                })?;
                template_shape = Some(shape);
                ActionPlan::InitCol(initcol)
            }
            ActionKind::Ctl => {
                let value = required_operand(operand, name)?;
                let ctl = parse_ctl_plan(value, line, line_index)?;
                let equals = value
                    .text()
                    .as_bytes()
                    .iter()
                    .position(|byte| *byte == b'=')
                    .expect("validated ctl envelope contains an equals delimiter");
                let ctl_bytes = value.text().len() - equals - 1;
                if ctl_bytes > build.limits.operands.max_ctl_value_bytes {
                    return Err(SemanticError::CtlValueByteLimit {
                        limit: build.limits.operands.max_ctl_value_bytes,
                        position: action_position(value, line, equals + 1),
                    });
                }
                ActionPlan::Ctl(ctl)
            }
            ActionKind::Tag | ActionKind::Ver | ActionKind::SkipAfter => {
                ActionPlan::Literal(required_operand(operand, name)?)
            }
        },
    };

    let position = operand.map_or(name.span().end, |value| value.span().start);
    let shape = template_shape.unwrap_or(TemplateShape {
        segments: 0,
        expansions: 0,
    });
    if template_shape.is_some() {
        if build
            .templates
            .len()
            .checked_add(1)
            .is_none_or(|count| count > build.limits.operands.max_templates)
        {
            return Err(SemanticError::TemplateLimit {
                limit: build.limits.operands.max_templates,
                position,
            });
        }
        if shape.expansions > build.limits.operands.max_expansions_per_template {
            return Err(SemanticError::TemplateExpansionLimit {
                limit: build.limits.operands.max_expansions_per_template,
                position,
            });
        }
        if build
            .segments
            .len()
            .checked_add(shape.segments)
            .is_none_or(|count| count > build.limits.operands.max_segments)
        {
            return Err(SemanticError::TemplateSegmentLimit {
                limit: build.limits.operands.max_segments,
                position,
            });
        }
        if build
            .expansions
            .len()
            .checked_add(shape.expansions)
            .is_none_or(|count| count > build.limits.operands.max_expansions)
        {
            return Err(SemanticError::ExpansionLimit {
                limit: build.limits.operands.max_expansions,
                position,
            });
        }
    }
    let structured = usize::from(matches!(
        plan,
        ActionPlan::SetVar(_) | ActionPlan::InitCol(_) | ActionPlan::Ctl(_)
    ));
    let structured_count =
        build
            .structured_count
            .checked_add(structured)
            .ok_or(SemanticError::StructuredActionLimit {
                limit: build.limits.operands.max_structured_actions,
                position,
            })?;
    if structured_count > build.limits.operands.max_structured_actions {
        return Err(SemanticError::StructuredActionLimit {
            limit: build.limits.operands.max_structured_actions,
            position,
        });
    }
    let ctl_target_count = usize::from(matches!(plan, ActionPlan::Ctl(CtlPlan::RuleRemoveTargetByTag { .. })));
    if build
        .ctl_targets
        .len()
        .checked_add(ctl_target_count)
        .is_none_or(|count| count > build.limits.operands.max_ctl_targets)
    {
        return Err(SemanticError::CtlTargetLimit {
            limit: build.limits.operands.max_ctl_targets,
            position,
        });
    }

    let runtime_macro = template_shape.is_some_and(|value| value.expansions > 0);
    let setvar_requirement = matches!(plan, ActionPlan::SetVar(_));
    let block_requirement = matches!(envelope, ActionEnvelopeKind::Action(ActionKind::Block));
    let requirement_count =
        usize::from(runtime_macro) + usize::from(setvar_requirement) + usize::from(block_requirement);
    let capability = match plan {
        ActionPlan::InitCol(_) => Some(UnavailableCapability::InitCol),
        ActionPlan::Ctl(CtlPlan::AuditEngine(_)) => Some(UnavailableCapability::Ctl),
        _ => None,
    };
    let dynamic = if template_shape.is_some() {
        size_of::<TemplateRecord<'_>>()
            .checked_add(size_of::<TemplateSegmentRecord>().checked_mul(shape.segments).ok_or(
                SemanticError::OwnedByteLimit {
                    limit: build.limits.max_owned_bytes,
                    position,
                },
            )?)
            .and_then(|value| value.checked_add(size_of::<ExpansionRecord<'_>>().checked_mul(shape.expansions)?))
    } else {
        Some(0)
    }
    .and_then(|value| {
        value.checked_add(match plan {
            ActionPlan::SetVar(_) => size_of::<SetVarRecord>(),
            ActionPlan::InitCol(_) => size_of::<InitColRecord>(),
            ActionPlan::Ctl(CtlPlan::RuleRemoveTargetByTag { .. }) => {
                size_of::<CtlRecord<'_>>() + size_of::<CtlTargetRecord<'_>>()
            }
            ActionPlan::Ctl(_) => size_of::<CtlRecord<'_>>(),
            _ => 0,
        })
    })
    .and_then(|value| value.checked_add(size_of::<RequirementRecord>().checked_mul(requirement_count)?))
    .and_then(|value| value.checked_add(size_of::<CapabilityRecord>().checked_mul(usize::from(capability.is_some()))?))
    .ok_or(SemanticError::OwnedByteLimit {
        limit: build.limits.max_owned_bytes,
        position,
    })?;
    let charge = project_dynamic(*build.semantic_charge, build.inherited, dynamic, build.limits, position)?;
    if build
        .requirements
        .len()
        .checked_add(requirement_count)
        .is_none_or(|count| count > build.requirements.capacity())
    {
        return Err(SemanticError::Allocation {
            operation: "preflight compile requirements",
        });
    }
    if build
        .unavailable
        .len()
        .checked_add(usize::from(capability.is_some()))
        .is_none_or(|count| count > build.unavailable.capacity())
    {
        return Err(SemanticError::Allocation {
            operation: "preflight unavailable capabilities",
        });
    }
    if template_shape.is_some() {
        build.templates.try_reserve(1).map_err(|_| SemanticError::Allocation {
            operation: "operand templates",
        })?;
        build
            .segments
            .try_reserve(shape.segments)
            .map_err(|_| SemanticError::Allocation {
                operation: "template segments",
            })?;
        build
            .expansions
            .try_reserve(shape.expansions)
            .map_err(|_| SemanticError::Allocation {
                operation: "macro expansions",
            })?;
    }
    match plan {
        ActionPlan::SetVar(_) => build.setvars.try_reserve(1).map_err(|_| SemanticError::Allocation {
            operation: "setvar records",
        })?,
        ActionPlan::InitCol(_) => build.initcols.try_reserve(1).map_err(|_| SemanticError::Allocation {
            operation: "initcol records",
        })?,
        ActionPlan::Ctl(CtlPlan::RuleRemoveTargetByTag { .. }) => {
            build.ctls.try_reserve(1).map_err(|_| SemanticError::Allocation {
                operation: "ctl records",
            })?;
            build
                .ctl_targets
                .try_reserve(1)
                .map_err(|_| SemanticError::Allocation {
                    operation: "ctl target records",
                })?;
        }
        ActionPlan::Ctl(_) => build.ctls.try_reserve(1).map_err(|_| SemanticError::Allocation {
            operation: "ctl records",
        })?,
        _ => {}
    }

    *build.semantic_charge = charge;
    *build.scanned = scanned;
    *build.structured_count = structured_count;
    let template = if template_shape.is_some() {
        let value = operand.expect("template action has a required operand");
        Some(push_template_records(
            value.text().bytes(),
            value.text().len(),
            OperandStorage::Borrowed(value),
            build.templates,
            build.segments,
            build.expansions,
            |range| {
                action_mapped_text(value, line, range).expect("action template mapping was validated before commit")
            },
        ))
    } else {
        None
    };
    let record = match plan {
        ActionPlan::NoArgument => ActionOperandRecord::NoArgument,
        ActionPlan::Local(value) => ActionOperandRecord::Local(value),
        ActionPlan::Transformation(value) => ActionOperandRecord::Transformation(value),
        ActionPlan::Template => ActionOperandRecord::Template(template.expect("template action prepared a template")),
        ActionPlan::Literal(value) => ActionOperandRecord::Literal(value),
        ActionPlan::SetVar(plan) => {
            let template = template.expect("setvar prepared a template");
            let value = operand.expect("setvar has an operand");
            let key = commit_template_slice(plan.key, template, value, line, build.templates, build.segments);
            let operand_value = plan
                .value
                .map(|slice| commit_template_slice(slice, template, value, line, build.templates, build.segments));
            let index = build.setvars.len();
            build.setvars.push(SetVarRecord {
                collection: CollectionKind::Tx,
                operation: plan.operation,
                key,
                value: operand_value,
                constant: plan.constant,
            });
            ActionOperandRecord::SetVar(index)
        }
        ActionPlan::InitCol(plan) => {
            let template = template.expect("initcol prepared a template");
            let value = operand.expect("initcol has an operand");
            let key = commit_template_slice(plan.key, template, value, line, build.templates, build.segments);
            let index = build.initcols.len();
            build.initcols.push(InitColRecord {
                collection: plan.collection,
                key,
            });
            ActionOperandRecord::InitCol(index)
        }
        ActionPlan::Ctl(plan) => {
            let ctl = match plan {
                CtlPlan::AuditEngine(value) => CtlRecord::AuditEngine(value),
                CtlPlan::ForceRequestBodyVariable(value) => CtlRecord::ForceRequestBodyVariable(value),
                CtlPlan::RequestBodyProcessor(value) => CtlRecord::RequestBodyProcessor(value),
                CtlPlan::RuleRemoveByTag(value) => CtlRecord::RuleRemoveByTag(value),
                CtlPlan::RuleRemoveTargetByTag { tag, target_range } => {
                    let value = operand.expect("ctl target action has an operand");
                    let target_text = action_mapped_text(value, line, target_range)
                        .expect("ctl target range was mapped before candidate commit");
                    let target =
                        parse_single_target(target_text.text(), target_text.logical_range().start, line, line_index)
                            .expect("ctl target grammar was validated before candidate commit");
                    let collection_spelling = SemanticText::from(target.collection());
                    let collection = CollectionKind::lookup_ascii_insensitive(collection_spelling.text().as_bytes())
                        .expect("ctl target collection was validated before commit");
                    let target_index = build.ctl_targets.len();
                    build.ctl_targets.push(CtlTargetRecord {
                        collection,
                        collection_spelling,
                        selector_kind: target.selector_kind(),
                        selector: target.selector().map(SemanticText::from),
                        regex_body: target.regex_body().map(SemanticText::from),
                        span: target.span(),
                    });
                    CtlRecord::RuleRemoveTargetByTag {
                        tag,
                        target: target_index,
                    }
                }
            };
            let index = build.ctls.len();
            build.ctls.push(ctl);
            ActionOperandRecord::Ctl(index)
        }
    };
    let operand_index = build.operands.len();
    build.operands.push(record);
    if runtime_macro {
        build.requirements.push(RequirementRecord {
            location,
            requirement: CompileRequirement::RuntimeMacro,
        });
    }
    if setvar_requirement {
        build.requirements.push(RequirementRecord {
            location,
            requirement: CompileRequirement::TransactionSetVar,
        });
    }
    if block_requirement {
        build.requirements.push(RequirementRecord {
            location,
            requirement: CompileRequirement::BlockDefaultDisruptive,
        });
    }
    if let Some(capability) = capability {
        build.unavailable.push(CapabilityRecord { location, capability });
    }
    Ok(operand_index)
}
