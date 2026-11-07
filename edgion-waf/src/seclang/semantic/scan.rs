use super::*;

pub(super) fn scan_semantics(draft: &mut SemanticDraft<'_>, limits: SemanticLimits) -> Result<(), SemanticError> {
    let SemanticDraft {
        documents,
        directives,
        targets,
        operators,
        actions,
        action_operands,
        setvars,
        initcols,
        ctls,
        ctl_targets,
        templates,
        template_segments,
        expansions,
        decoded_operands,
        operator_operands,
        static_operands,
        list_items,
        networks,
        phrase_count,
        asset_count,
        structured_action_count,
        phrase_bytes,
        scanned_operand_bytes,
        operand_limits: _,
        requirements,
        unavailable,
        folds: _,
        effective_transformations: _,
        final_rules: _,
        chains: _,
        chain_members: _,
        markers: _,
        resolved_skip_afters: _,
        semantic_charged_bytes,
        inherited_charged_bytes,
    } = draft;
    for (document_index, document) in documents.iter().enumerate() {
        for directive_index in 0..document.len() {
            let view = document
                .get(directive_index)
                .expect("semantic scan index is bounded by immutable syntax document length");
            let target_start = targets.len();
            let operator_start = operators.len();
            let action_start = actions.len();
            match view.targets() {
                DirectiveTargetView::None | DirectiveTargetView::Remove { .. } => {}
                DirectiveTargetView::Rule { atoms } | DirectiveTargetView::Update { atoms, .. } => {
                    for atom in atoms {
                        let collection_spelling = SemanticText::from(atom.collection());
                        let collection = CollectionKind::lookup_ascii_insensitive(
                            collection_spelling.text().as_bytes(),
                        )
                        .ok_or(SemanticError::UnknownCollection {
                            position: collection_spelling.span().start,
                        })?;
                        let location =
                            location(document_index, directive_index, SemanticNodeKind::Target, targets.len());
                        targets.push(SemanticTarget {
                            location,
                            collection,
                            collection_spelling,
                            selector_kind: atom.selector_kind(),
                            modifier: atom.modifier(),
                            selector: atom.selector().map(SemanticText::from),
                            regex_body: atom.regex_body().map(SemanticText::from),
                            span: atom.span(),
                        });
                        if collection == CollectionKind::ResponseBody && atom.modifier() != TargetModifier::Exclude {
                            push_capability(
                                unavailable,
                                semantic_charged_bytes,
                                *inherited_charged_bytes,
                                location,
                                UnavailableCapability::FullResponseBody,
                                limits,
                                collection_spelling.span().start,
                            )?;
                        }
                    }
                }
            }
            if let Some(operator) = view.operator() {
                let (kind, spelling) = match operator.name() {
                    OperatorNameView::ImplicitRx => (OperatorKind::Rx, None),
                    OperatorNameView::Explicit(name) => {
                        let kind = lookup_operator(name).ok_or(SemanticError::UnknownOperator {
                            position: operator.name_span().expect("explicit name has span").start,
                        })?;
                        let range = operator.name_logical_range().expect("explicit name has range");
                        (
                            kind,
                            Some(SemanticText {
                                text: name.raw(),
                                logical_start: range.start,
                                logical_end: range.end,
                                span: operator.name_span().expect("explicit name has span"),
                            }),
                        )
                    }
                };
                let location = location(
                    document_index,
                    directive_index,
                    SemanticNodeKind::Operator,
                    operators.len(),
                );
                let line = &document.source().lines()[operator.logical_line_index()];
                let operand = classify_operator_operand(
                    &mut OperatorOperandBuild {
                        templates: TemplateBuild {
                            templates,
                            segments: template_segments,
                            expansions,
                            decoded: decoded_operands,
                            requirements,
                            semantic_charge: semantic_charged_bytes,
                            scanned: scanned_operand_bytes,
                            inherited: *inherited_charged_bytes,
                            limits,
                        },
                        operands: operator_operands,
                        static_operands,
                        list_items,
                        networks,
                        phrase_count,
                        asset_count,
                        phrase_bytes,
                        unavailable,
                    },
                    operator,
                    line,
                    kind,
                    location,
                    document_index,
                )?;
                operators.push(SemanticOperatorRecord {
                    location,
                    kind,
                    spelling,
                    negated: operator.negated(),
                    operand,
                });
            }
            if let Some(action_list) = view.actions() {
                for entry in action_list.entries() {
                    let name = SemanticText::from(entry.name());
                    let location = location(document_index, directive_index, SemanticNodeKind::Action, actions.len());
                    let operand = entry.value().map(|value| SemanticText::from(value.body()));
                    let kind = if entry.kind() == ActionEntryKind::Transformation {
                        ActionEnvelopeKind::Transformation
                    } else {
                        let action = ActionKind::lookup_ascii_insensitive(name.text().as_bytes()).ok_or(
                            SemanticError::UnknownAction {
                                position: name.span().start,
                            },
                        )?;
                        ActionEnvelopeKind::Action(action)
                    };
                    let line_index = action_list.logical_line_index();
                    let line = &document.source().lines()[line_index];
                    let operand = classify_action_operand(
                        &mut ActionOperandBuild {
                            templates,
                            segments: template_segments,
                            expansions,
                            operands: action_operands,
                            setvars,
                            initcols,
                            ctls,
                            ctl_targets,
                            structured_count: structured_action_count,
                            requirements,
                            unavailable,
                            semantic_charge: semantic_charged_bytes,
                            scanned: scanned_operand_bytes,
                            inherited: *inherited_charged_bytes,
                            limits,
                        },
                        kind,
                        operand,
                        name,
                        line,
                        line_index,
                        location,
                    )?;
                    actions.push(SemanticActionRecord {
                        location,
                        kind,
                        name,
                        operand,
                    });
                }
            }
            directives.push(DirectiveSemanticRecord {
                document_index,
                directive_index,
                target_start,
                target_count: targets.len() - target_start,
                operator: (operators.len() != operator_start).then_some(operator_start),
                action_start,
                action_count: actions.len() - action_start,
                fold: None,
            });
        }
    }
    Ok(())
}

fn lookup_operator(value: DecodedTextView<'_, '_>) -> Option<OperatorKind> {
    [
        OperatorKind::BeginsWith,
        OperatorKind::Contains,
        OperatorKind::DetectSqli,
        OperatorKind::DetectXss,
        OperatorKind::EndsWith,
        OperatorKind::Eq,
        OperatorKind::Ge,
        OperatorKind::Gt,
        OperatorKind::IpMatch,
        OperatorKind::Lt,
        OperatorKind::Pm,
        OperatorKind::PmFromFile,
        OperatorKind::Rx,
        OperatorKind::Streq,
        OperatorKind::UnconditionalMatch,
        OperatorKind::ValidateByteRange,
        OperatorKind::ValidateUrlEncoding,
        OperatorKind::ValidateUtf8Encoding,
        OperatorKind::Within,
    ]
    .into_iter()
    .find(|kind| {
        let canonical = OperatorKind::lookup_ascii_insensitive(kind.as_str().as_bytes())
            .expect("operator registry contains its canonical spelling");
        value.bytes().eq(canonical.as_str().bytes())
    })
}

pub(super) fn required_operand<'source>(
    operand: Option<SemanticText<'source>>,
    name: SemanticText<'_>,
) -> Result<SemanticText<'source>, SemanticError> {
    match operand {
        Some(value) if !value.text().is_empty() => Ok(value),
        _ => Err(SemanticError::ValueContract {
            expected: ValueContract::Required,
            actual: ValueContract::Forbidden,
            position: name.span().end,
        }),
    }
}

pub(super) fn forbidden_operand(
    operand: Option<SemanticText<'_>>,
    name: SemanticText<'_>,
) -> Result<(), SemanticError> {
    if operand.is_some() {
        Err(SemanticError::ValueContract {
            expected: ValueContract::Forbidden,
            actual: ValueContract::Required,
            position: name.span().end,
        })
    } else {
        Ok(())
    }
}

fn scalar_bytes<'source>(value: SemanticText<'source>, kind: ScalarKind) -> Result<&'source [u8], SemanticError> {
    if value.text().contains("%{") {
        return Err(SemanticError::MacroScalar {
            kind,
            position: value.span().start,
        });
    }
    if value.text().is_empty() {
        return Err(SemanticError::InvalidScalar {
            kind,
            position: value.span().start,
        });
    }
    Ok(value.text().as_bytes())
}

fn parse_unsigned(value: SemanticText<'_>, kind: ScalarKind) -> Result<u64, SemanticError> {
    let bytes = scalar_bytes(value, kind)?;
    if !bytes.iter().all(u8::is_ascii_digit) {
        return Err(SemanticError::InvalidScalar {
            kind,
            position: value.span().start,
        });
    }
    let mut parsed = 0_u64;
    for byte in bytes {
        parsed = parsed
            .checked_mul(10)
            .and_then(|number| number.checked_add(u64::from(byte - b'0')))
            .ok_or(SemanticError::ScalarOverflow {
                kind,
                position: value.span().start,
            })?;
    }
    Ok(parsed)
}

pub(super) fn parse_id(value: SemanticText<'_>) -> Result<LocalActionValue, SemanticError> {
    let parsed = parse_unsigned(value, ScalarKind::Id)?;
    if parsed == 0 {
        return Err(SemanticError::InvalidScalar {
            kind: ScalarKind::Id,
            position: value.span().start,
        });
    }
    Ok(LocalActionValue::Id(parsed))
}

pub(super) fn parse_phase(value: SemanticText<'_>) -> Result<LocalActionValue, SemanticError> {
    let phase = match value.text().as_bytes() {
        b"request" => Phase::RequestBody,
        b"response" => Phase::ResponseBody,
        b"logging" => Phase::Logging,
        _ => match parse_unsigned(value, ScalarKind::Phase)? {
            1 => Phase::RequestHeaders,
            2 => Phase::RequestBody,
            3 => Phase::ResponseHeaders,
            4 => Phase::ResponseBody,
            5 => Phase::Logging,
            _ => {
                return Err(SemanticError::InvalidScalar {
                    kind: ScalarKind::Phase,
                    position: value.span().start,
                })
            }
        },
    };
    Ok(LocalActionValue::Phase(phase))
}

pub(super) fn parse_severity(value: SemanticText<'_>) -> Result<LocalActionValue, SemanticError> {
    let bytes = scalar_bytes(value, ScalarKind::Severity)?;
    let severity = if bytes.len() == 1 && matches!(bytes[0], b'0'..=b'7') {
        severity_from_number(bytes[0] - b'0')
    } else {
        [
            Severity::Emergency,
            Severity::Alert,
            Severity::Critical,
            Severity::Error,
            Severity::Warning,
            Severity::Notice,
            Severity::Info,
            Severity::Debug,
        ]
        .into_iter()
        .find(|severity| bytes.eq_ignore_ascii_case(severity_name(*severity).as_bytes()))
        .ok_or(SemanticError::InvalidScalar {
            kind: ScalarKind::Severity,
            position: value.span().start,
        })?
    };
    Ok(LocalActionValue::Severity(severity))
}

fn severity_from_number(value: u8) -> Severity {
    match value {
        0 => Severity::Emergency,
        1 => Severity::Alert,
        2 => Severity::Critical,
        3 => Severity::Error,
        4 => Severity::Warning,
        5 => Severity::Notice,
        6 => Severity::Info,
        _ => Severity::Debug,
    }
}

fn severity_name(value: Severity) -> &'static str {
    match value {
        Severity::Emergency => "emergency",
        Severity::Alert => "alert",
        Severity::Critical => "critical",
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Notice => "notice",
        Severity::Info => "info",
        Severity::Debug => "debug",
    }
}

pub(super) fn parse_status(value: SemanticText<'_>) -> Result<LocalActionValue, SemanticError> {
    let parsed = parse_unsigned(value, ScalarKind::Status)?;
    if !(200..=599).contains(&parsed) {
        return Err(SemanticError::InvalidScalar {
            kind: ScalarKind::Status,
            position: value.span().start,
        });
    }
    Ok(LocalActionValue::Status(parsed as u16))
}

pub(super) fn parse_skip(value: SemanticText<'_>) -> Result<LocalActionValue, SemanticError> {
    let parsed = parse_unsigned(value, ScalarKind::Skip)?;
    if parsed == 0 {
        return Err(SemanticError::InvalidScalar {
            kind: ScalarKind::Skip,
            position: value.span().start,
        });
    }
    let parsed = u32::try_from(parsed).map_err(|_| SemanticError::ScalarOverflow {
        kind: ScalarKind::Skip,
        position: value.span().start,
    })?;
    Ok(LocalActionValue::Skip(parsed))
}

fn push_capability(
    records: &mut Vec<CapabilityRecord>,
    semantic_charged_bytes: &mut usize,
    inherited_charged_bytes: usize,
    location: SemanticLocation,
    capability: UnavailableCapability,
    limits: SemanticLimits,
    position: SourcePosition,
) -> Result<(), SemanticError> {
    charge_dynamic::<CapabilityRecord>(semantic_charged_bytes, inherited_charged_bytes, limits, position)?;
    records.push(CapabilityRecord { location, capability });
    Ok(())
}

fn charge_dynamic<T>(
    semantic_charged_bytes: &mut usize,
    inherited_charged_bytes: usize,
    limits: SemanticLimits,
    position: SourcePosition,
) -> Result<(), SemanticError> {
    let semantic = (*semantic_charged_bytes)
        .checked_add(size_of::<T>())
        .ok_or(SemanticError::OwnedByteLimit {
            limit: limits.max_owned_bytes,
            position,
        })?;
    if semantic > limits.max_owned_bytes {
        return Err(SemanticError::OwnedByteLimit {
            limit: limits.max_owned_bytes,
            position,
        });
    }
    let retained = inherited_charged_bytes
        .checked_add(semantic)
        .ok_or(SemanticError::RetainedByteLimit {
            limit: limits.max_retained_bytes,
        })?;
    if retained > limits.max_retained_bytes {
        return Err(SemanticError::RetainedByteLimit {
            limit: limits.max_retained_bytes,
        });
    }
    *semantic_charged_bytes = semantic;
    Ok(())
}
