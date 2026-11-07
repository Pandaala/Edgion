use super::*;

pub(super) fn map_phrase_matcher_error(error: MatcherCompileError, location: SemanticLocation) -> CompilerError {
    let kind = match error {
        MatcherCompileError::PeakArithmetic => PhraseMatcherErrorKind::Arithmetic,
        MatcherCompileError::Allocation { .. } => PhraseMatcherErrorKind::Allocation,
        MatcherCompileError::PatternLimit { .. } => PhraseMatcherErrorKind::PatternLimit,
        MatcherCompileError::PatternByteLimit { .. } => PhraseMatcherErrorKind::PatternByteLimit,
        MatcherCompileError::EmptyPhrase { .. } => PhraseMatcherErrorKind::EmptyPhrase,
        MatcherCompileError::PhraseStateLimit { .. } => PhraseMatcherErrorKind::StateLimit,
        MatcherCompileError::PhraseTransitionLimit { .. } => PhraseMatcherErrorKind::TransitionLimit,
        MatcherCompileError::PhraseOutputLimit { .. } => PhraseMatcherErrorKind::OutputLimit,
        MatcherCompileError::PhraseCompiledByteLimit { .. } => PhraseMatcherErrorKind::CompiledByteLimit,
        MatcherCompileError::PhraseBuild { .. }
        | MatcherCompileError::Regex { .. }
        | MatcherCompileError::RegexSourceByteLimit { .. }
        | MatcherCompileError::RegexSourceGroupByteLimit { .. }
        | MatcherCompileError::RegexProgramByteLimit { .. } => PhraseMatcherErrorKind::Build,
    };
    CompilerError::PhraseMatcher { location, kind }
}

/// Reads only the resolved assets, phrase records and phrase bytes of `templates`.
fn visit_phrase_operand(
    operand: super::super::OperatorOperandView<'_, '_>,
    templates: &AssetTemplateState<'_>,
    asset_cursor: &mut usize,
    mut visit: impl FnMut(&str, usize) -> Result<(), CompilerError>,
) -> Result<usize, CompilerError> {
    let (assets, phrases, asset_bytes) = (&templates.assets, &templates.phrases, &templates.asset_phrase_bytes);
    let mut ordinal = 0_usize;
    match operand {
        super::super::OperatorOperandView::PhraseList(list) => {
            for phrase in list.phrases() {
                visit(phrase.text(), ordinal)?;
                ordinal = checked_inc(ordinal)?;
            }
        }
        super::super::OperatorOperandView::ExternalAssets(external) => {
            for _ in external.assets() {
                let asset = assets.get(*asset_cursor).ok_or(CompilerError::PrivateInvariant {
                    kind: CompilerInvariant::InvalidByteRange,
                })?;
                if asset.request_id != *asset_cursor {
                    return Err(CompilerError::PrivateInvariant {
                        kind: CompilerInvariant::InvalidByteRange,
                    });
                }
                let end = asset
                    .phrase_start
                    .checked_add(asset.phrase_count)
                    .ok_or(CompilerError::PreflightArithmetic)?;
                for phrase in phrases
                    .get(asset.phrase_start..end)
                    .ok_or(CompilerError::PrivateInvariant {
                        kind: CompilerInvariant::InvalidByteRange,
                    })?
                {
                    let bytes = byte_range(asset_bytes, phrase.bytes)?;
                    let text = std::str::from_utf8(bytes).map_err(|_| CompilerError::PrivateInvariant {
                        kind: CompilerInvariant::InvalidByteRange,
                    })?;
                    visit(text, ordinal)?;
                    ordinal = checked_inc(ordinal)?;
                }
                *asset_cursor = checked_inc(*asset_cursor)?;
            }
        }
        _ => {
            return Err(CompilerError::PrivateInvariant {
                kind: CompilerInvariant::UnexpectedActionOperand,
            });
        }
    }
    Ok(ordinal)
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct GroupPatternPreflight {
    matcher_members: usize,
    grouped_patterns: usize,
    grouped_source_bytes: usize,
    active_capture_phrase_programs: usize,
    pub(super) active_patterns: usize,
    pub(super) active_source_bytes: usize,
}

impl GroupPatternPreflight {
    fn inspect(state: &MatcherState<'_>, limits: CompilerLimits) -> Result<Self, CompilerError> {
        let mut result = Self::default();
        let mut asset_cursor = 0_usize;
        for unit in state.asset_templates.validated.rule_units() {
            if unit.is_compatibility_excluded() {
                continue;
            }
            for member in unit.members() {
                let Some(operator) = member.operator() else {
                    continue;
                };
                let capture = member.rule().fold().capture_action().is_some();
                match operator.kind() {
                    super::super::OperatorKind::Pm | super::super::OperatorKind::PmFromFile => {
                        let mut patterns = 0_usize;
                        let mut bytes = 0_usize;
                        visit_phrase_operand(
                            operator.operand(),
                            &state.asset_templates,
                            &mut asset_cursor,
                            |phrase, _| {
                                patterns = checked_inc(patterns)?;
                                bytes = checked_add(bytes, phrase.len())?;
                                Ok(())
                            },
                        )?;
                        if unit.is_active() {
                            result.active_patterns = checked_add(result.active_patterns, patterns)?;
                            result.active_source_bytes = checked_add(result.active_source_bytes, bytes)?;
                            if capture {
                                result.active_capture_phrase_programs =
                                    checked_inc(result.active_capture_phrase_programs)?;
                            } else {
                                result.matcher_members = checked_inc(result.matcher_members)?;
                                result.grouped_patterns = checked_add(result.grouped_patterns, patterns)?;
                                result.grouped_source_bytes = checked_add(result.grouped_source_bytes, bytes)?;
                            }
                        }
                    }
                    super::super::OperatorKind::Rx if unit.is_active() && !capture => {
                        let super::super::OperatorOperandView::Regex(source) = operator.operand() else {
                            return Err(CompilerError::PrivateInvariant {
                                kind: CompilerInvariant::UnexpectedActionOperand,
                            });
                        };
                        result.matcher_members = checked_inc(result.matcher_members)?;
                        result.grouped_patterns = checked_inc(result.grouped_patterns)?;
                        result.grouped_source_bytes = checked_add(result.grouped_source_bytes, source.text().len())?;
                        result.active_patterns = checked_inc(result.active_patterns)?;
                        result.active_source_bytes = checked_add(result.active_source_bytes, source.text().len())?;
                    }
                    _ => {}
                }
            }
        }
        if asset_cursor != state.asset_templates.assets.len() {
            return Err(CompilerError::PrivateInvariant {
                kind: CompilerInvariant::InvalidByteRange,
            });
        }
        if result.active_patterns > limits.max_matcher_patterns {
            return Err(CompilerError::MatcherPatternLimit {
                actual: result.active_patterns,
                limit: limits.max_matcher_patterns,
            });
        }
        if result.active_source_bytes > limits.max_matcher_source_bytes {
            return Err(CompilerError::MatcherSourceByteLimit {
                actual: result.active_source_bytes,
                limit: limits.max_matcher_source_bytes,
            });
        }
        Ok(result)
    }
}

// Independent outputs: the pattern arena and three per-group link tables are
// distinct caller-owned tables; group, kind, source and mapping vary per call.
#[allow(clippy::too_many_arguments)]
fn append_candidate_pattern(
    group: usize,
    kind: CandidatePatternKind,
    source: &str,
    mapping: CompiledPatternMapping,
    pattern_bytes: &mut Vec<u8>,
    patterns: &mut Vec<CandidatePattern>,
    heads: &mut [usize],
    tails: &mut [usize],
    counts: &mut [usize],
) -> Result<(), CompilerError> {
    let start = pattern_bytes.len();
    pattern_bytes.extend_from_slice(source.as_bytes());
    let index = patterns.len();
    patterns.push(CandidatePattern {
        group,
        kind,
        bytes: ByteRange {
            start,
            len: source.len(),
        },
        mapping,
        next: usize::MAX,
    });
    let head = heads.get_mut(group).ok_or(CompilerError::PrivateInvariant {
        kind: CompilerInvariant::InvalidByteRange,
    })?;
    let tail = tails.get_mut(group).ok_or(CompilerError::PrivateInvariant {
        kind: CompilerInvariant::InvalidByteRange,
    })?;
    let count = counts.get_mut(group).ok_or(CompilerError::PrivateInvariant {
        kind: CompilerInvariant::InvalidByteRange,
    })?;
    if *tail == usize::MAX {
        *head = index;
    } else {
        patterns
            .get_mut(*tail)
            .ok_or(CompilerError::PrivateInvariant {
                kind: CompilerInvariant::InvalidByteRange,
            })?
            .next = index;
    }
    *tail = index;
    *count = checked_inc(*count)?;
    Ok(())
}

fn compile_phrase_operand_program(
    operand: super::super::OperatorOperandView<'_, '_>,
    templates: &AssetTemplateState<'_>,
    asset_cursor: &mut usize,
    location: SemanticLocation,
    retain: bool,
    work: &mut MatcherCompileWork,
) -> Result<CompiledCapturePhraseProgram, CompilerError> {
    let mut count_cursor = *asset_cursor;
    let mut pattern_count = 0_usize;
    let mut source_bytes = 0_usize;
    visit_phrase_operand(operand.clone(), templates, &mut count_cursor, |phrase, _| {
        pattern_count = checked_inc(pattern_count)?;
        source_bytes = checked_add(source_bytes, phrase.len())?;
        Ok(())
    })?;
    work.retain_working_bytes(source_bytes)?;
    work.retain_working_records::<ByteRange>(pattern_count)?;
    work.retain_working_records::<&str>(pattern_count)?;
    let mut owned = Vec::new();
    let mut ranges = Vec::new();
    owned
        .try_reserve_exact(source_bytes)
        .map_err(|_| CompilerError::Allocation {
            operation: "capture phrase source bytes",
        })?;
    reserve_exact(&mut ranges, pattern_count, "capture phrase ranges")?;
    visit_phrase_operand(operand, templates, asset_cursor, |phrase, _| {
        let start = owned.len();
        owned.extend_from_slice(phrase.as_bytes());
        ranges.push(ByteRange {
            start,
            len: phrase.len(),
        });
        Ok(())
    })?;
    if *asset_cursor != count_cursor || owned.len() != source_bytes || ranges.len() != pattern_count {
        return Err(CompilerError::PrivateInvariant {
            kind: CompilerInvariant::InvalidByteRange,
        });
    }
    let mut refs = Vec::new();
    reserve_exact(&mut refs, pattern_count, "capture phrase references")?;
    for range in &ranges {
        refs.push(
            std::str::from_utf8(byte_range(&owned, *range)?).map_err(|_| CompilerError::PrivateInvariant {
                kind: CompilerInvariant::InvalidByteRange,
            })?,
        );
    }
    let program = work.compile_capture_phrase(&refs, source_bytes, location, retain)?;
    work.drop_working_records::<&str>(pattern_count);
    work.drop_working_records::<ByteRange>(pattern_count);
    work.drop_working_bytes(source_bytes);
    Ok(program)
}

pub(super) struct CompilerBuilder<'source, 'assets> {
    pub(super) external_live_bytes: usize,
    validated: ValidatedSemanticDocument<'source>,
    assets: Option<&'assets mut dyn PhraseAssetSource>,
    limits: CompilerLimits,
}

impl<'source, 'assets> CompilerBuilder<'source, 'assets> {
    pub(super) fn new(
        validated: ValidatedSemanticDocument<'source>,
        assets: Option<&'assets mut dyn PhraseAssetSource>,
        limits: CompilerLimits,
    ) -> Self {
        Self {
            external_live_bytes: 0,
            validated,
            assets,
            limits,
        }
    }

    pub(super) fn prepare(self) -> Result<AssetTemplateState<'source>, CompilerError> {
        let compilation_input_bytes = checked_add(self.validated.retained_charged_bytes(), self.external_live_bytes)?;
        let unit_count = self.validated.rule_units().len();
        let node_count = self.validated.rule_nodes().len();
        if unit_count > self.limits.max_units {
            return Err(CompilerError::UnitLimit {
                actual: unit_count,
                limit: self.limits.max_units,
            });
        }
        if node_count > self.limits.max_nodes {
            return Err(CompilerError::NodeLimit {
                actual: node_count,
                limit: self.limits.max_nodes,
            });
        }
        let preflight = KnownPreflight::inspect(&self.validated, self.limits)?;
        preflight.validate(self.limits)?;
        for requirement in self.validated.requirements() {
            if matches!(
                requirement.kind(),
                CompileRequirement::BlockDefaultDisruptive | CompileRequirement::DefaultPhase
            ) {
                return Err(CompilerError::PrivateInvariant {
                    kind: CompilerInvariant::DefaultRequirement,
                });
            }
        }
        preflight.validate_peak(
            compilation_input_bytes,
            self.validated.requirements().len(),
            self.limits,
        )?;
        let mut requirements = Vec::new();
        requirements
            .try_reserve_exact(self.validated.requirements().len())
            .map_err(|_| CompilerError::Allocation {
                operation: "requirement discharge",
            })?;
        for requirement in self.validated.requirements() {
            requirements.push(RequirementState {
                kind: requirement.kind(),
                location: requirement.location(),
                discharged: false,
            });
        }
        requirements.sort_unstable_by_key(|requirement| requirement_key(requirement.kind, requirement.location));
        let mut work = CompileWork::new(compilation_input_bytes, self.limits, requirements, preflight)?;
        let mut assets = self.assets;
        for unit in self.validated.rule_units() {
            if unit.is_compatibility_excluded() {
                continue;
            }
            for member in unit.members() {
                if let Some(operator) = member.operator() {
                    if let super::super::OperatorOperandView::ExternalAssets(external) = operator.operand() {
                        for asset in external.assets() {
                            work.resolve_asset(operator.location(), asset, &mut assets)?;
                        }
                        work.discharge(CompileRequirement::ExternalPhraseAsset, operator.location())?;
                    }
                }
            }
        }
        for unit in self.validated.rule_units() {
            for member in unit.members() {
                work.compile_member(member, unit.is_active(), unit.is_compatibility_excluded())?;
            }
        }
        work.verify_a_requirements()?;
        Ok(AssetTemplateState {
            compilation_input_bytes,
            validated: self.validated,
            bytes: work.bytes,
            asset_phrase_bytes: work.asset_phrase_bytes,
            templates: work.templates,
            segments: work.segments,
            macros: work.macros,
            actions: work.actions,
            setvars: work.setvars,
            members: work.members,
            transformations: work.transformations,
            assets: work.assets,
            phrases: work.phrases,
            requirements: work.requirements,
            compiler_owned_bytes: work.owned_bytes,
            compiler_working_bytes: work.working_bytes,
            peak_bytes: work.peak_bytes,
        })
    }
}

impl<'source> MatcherState<'source> {
    pub(super) fn compile_with_work(
        mut state: AssetTemplateState<'source>,
        limits: CompilerLimits,
        mut canonical_work: CanonicalWork,
    ) -> Result<Self, CompilerError> {
        let target_regex_count = state
            .requirements
            .iter()
            .filter(|requirement| requirement.kind == CompileRequirement::TargetSelectorRegexCompilation)
            .count();
        let mut regex_counts = RegexCandidateCounts {
            selectors: target_regex_count,
            ..RegexCandidateCounts::default()
        };
        for unit in state.validated.rule_units() {
            if unit.is_compatibility_excluded() {
                continue;
            }
            for member in unit.members() {
                let Some(operator) = member.operator() else {
                    continue;
                };
                if operator.kind() != super::super::OperatorKind::Rx {
                    continue;
                }
                let capture = member.rule().fold().capture_action().is_some();
                match (unit.is_active(), capture) {
                    (true, false) => regex_counts.active_boolean += 1,
                    (true, true) => regex_counts.active_capture += 1,
                    (false, false) => regex_counts.removed_boolean += 1,
                    (false, true) => regex_counts.removed_capture += 1,
                }
            }
        }
        let active_regex_count = regex_counts.active().ok_or(CompilerError::PreflightArithmetic)?;
        if active_regex_count > limits.max_regex_programs {
            return Err(CompilerError::RegexProgramLimit {
                actual: active_regex_count,
                limit: limits.max_regex_programs,
            });
        }
        let ip_count = state
            .validated
            .rule_units()
            .filter(|unit| unit.is_active())
            .flat_map(|unit| unit.members())
            .filter_map(|member| member.operator())
            .map(|operator| match operator.operand() {
                super::super::OperatorOperandView::IpNetworks(networks) => networks.networks().len(),
                _ => 0,
            })
            .try_fold(0_usize, |total, count| total.checked_add(count))
            .ok_or(CompilerError::PreflightArithmetic)?;
        let target_identity_slots = state
            .validated
            .active_rule_units()
            .flat_map(|unit| unit.members())
            .flat_map(|member| member.final_targets())
            .filter(|target| target.regex_body().is_some())
            .map(|target| target.location().node_index().saturating_add(1))
            .max()
            .unwrap_or(0);

        let mut work = MatcherCompileWork {
            limits,
            validated_bytes: state.compilation_input_bytes,
            working_bytes: state.compiler_working_bytes,
            compiler_owned_bytes: state.compiler_owned_bytes,
            matcher_charged_bytes: 0,
            peak_bytes: state.peak_bytes,
            requirement_cursors: [0; 8],
        };
        work.retain_compiler_records::<Option<CompiledOperatorPlan>>(state.members.len())?;
        work.retain_compiler_records::<CompiledIpNetwork>(ip_count)?;
        work.retain_compiler_records::<CompiledTargetRegexPlan>(target_regex_count)?;
        work.retain_compiler_records::<CompiledRegexProgram>(target_regex_count)?;
        work.retain_compiler_records::<CompiledCaptureRegexProgram>(regex_counts.active_capture)?;
        work.retain_working_records::<usize>(target_identity_slots)?;
        let candidate_count = regex_counts.total().ok_or(CompilerError::PreflightArithmetic)?;
        work.retain_working_records::<RegexCandidate<'_>>(candidate_count)?;

        let mut operator_plans = Vec::new();
        let mut ip_networks = Vec::new();
        let mut target_regexes = Vec::new();
        let mut boolean_regexes = Vec::new();
        let mut capture_regexes = Vec::new();
        reserve_exact(&mut operator_plans, state.members.len(), "operator plans")?;
        reserve_exact(&mut ip_networks, ip_count, "compiled IP networks")?;
        reserve_exact(&mut target_regexes, target_regex_count, "target regex plans")?;
        reserve_exact(&mut boolean_regexes, target_regex_count, "boolean regex programs")?;
        reserve_exact(
            &mut capture_regexes,
            regex_counts.active_capture,
            "capture regex programs",
        )?;
        let mut regex_candidates = Vec::new();
        reserve_exact(&mut regex_candidates, candidate_count, "regex candidates")?;
        let mut target_ids = Vec::new();
        reserve_exact(&mut target_ids, target_identity_slots, "target regex identity map")?;
        target_ids.resize(target_identity_slots, usize::MAX);

        for unit in state.validated.active_rule_units() {
            for member in unit.members() {
                for target in member.final_targets() {
                    if let Some(body) = target.regex_body() {
                        let identity = target.location().node_index();
                        if target_ids[identity] != usize::MAX {
                            continue;
                        }
                        target_ids[identity] = 0;
                        regex_candidates.push(RegexCandidate {
                            source: body.text(),
                            location: target.location(),
                            capture: false,
                            active: true,
                        });
                    }
                }
            }
        }
        for unit in state.validated.rule_units() {
            if unit.is_compatibility_excluded() {
                continue;
            }
            for member in unit.members() {
                if let Some(operator) = member.operator() {
                    if let super::super::OperatorOperandView::Regex(source) = operator.operand() {
                        regex_candidates.push(RegexCandidate {
                            source: source.text(),
                            location: operator.location(),
                            capture: member.rule().fold().capture_action().is_some(),
                            active: unit.is_active(),
                        });
                    }
                }
            }
        }
        debug_assert_eq!(regex_candidates.len(), candidate_count);
        target_ids.fill(usize::MAX);

        let mut selector_interner = RegexProgramInterner::new(target_regex_count, &mut work)?;
        for unit in state.validated.active_rule_units() {
            for member in unit.members() {
                for target in member.final_targets() {
                    let Some(body) = target.regex_body() else {
                        continue;
                    };
                    let identity = target.location().node_index();
                    if target_ids[identity] != usize::MAX {
                        continue;
                    }
                    let ascii_case_insensitive = target.collection() == CollectionKind::Tx;
                    let program_id = selector_interner.compile_with(
                        body.text(),
                        ascii_case_insensitive,
                        &mut boolean_regexes,
                        &mut work,
                        &mut canonical_work,
                        |work| {
                            if ascii_case_insensitive {
                                work.compile_ascii_case_insensitive_boolean_regex(body.text(), target.location(), true)
                            } else {
                                work.compile_boolean_regex(body.text(), target.location(), true)
                            }
                        },
                    )?;
                    let plan_id = target_regexes.len();
                    target_regexes.push(CompiledTargetRegexPlan {
                        identity: target.location(),
                        program: program_id,
                    });
                    target_ids[identity] = plan_id;
                    discharge_requirement(
                        &mut state.requirements,
                        &mut work.requirement_cursors,
                        CompileRequirement::TargetSelectorRegexCompilation,
                        target.location(),
                    )?;
                }
            }
        }

        selector_interner.release(&mut work);
        let mut capture_interner = RegexProgramInterner::new(regex_counts.active_capture, &mut work)?;
        let mut member_index = 0_usize;
        for unit in state.validated.rule_units() {
            for member in unit.members() {
                let plan = if unit.is_compatibility_excluded() {
                    None
                } else if let Some(operator) = member.operator() {
                    compile_operator_plan(
                        operator,
                        member,
                        state.members[member_index].operator_template,
                        &mut ip_networks,
                        &mut boolean_regexes,
                        &mut capture_regexes,
                        &mut capture_interner,
                        &mut canonical_work,
                        &mut state.requirements,
                        &mut work,
                        unit.is_active(),
                    )?
                } else {
                    None
                };
                operator_plans.push(plan);
                member_index += 1;
            }
        }
        debug_assert_eq!(member_index, state.members.len());
        verify_b_requirements(&state.requirements)?;
        debug_assert!(regex_candidates.iter().all(|candidate| {
            !candidate.source.is_empty()
                || candidate.location.node_kind() == super::super::SemanticNodeKind::Operator
                || !candidate.capture
                || candidate.active
        }));
        capture_interner.release(&mut work);
        work.drop_working_records::<RegexCandidate<'_>>(candidate_count);
        work.drop_working_records::<usize>(target_identity_slots);
        Ok(Self {
            asset_templates: state,
            operator_plans,
            ip_networks,
            target_regexes,
            boolean_regexes,
            capture_regexes,
            canonical_work,
            compiler_owned_bytes: work.compiler_owned_bytes,
            matcher_charged_bytes: work.matcher_charged_bytes,
            peak_bytes: work.peak_bytes,
        })
    }
}

impl<'source> GroupedMatcherState<'source> {
    pub(super) fn compile(mut state: MatcherState<'source>, limits: CompilerLimits) -> Result<Self, CompilerError> {
        let mut active_members = 0_usize;
        let mut target_records_count = 0_usize;
        let mut target_selector_bytes = 0_usize;
        let mut target_identity_slots = 0_usize;
        for unit in state.asset_templates.validated.active_rule_units() {
            for member in unit.members() {
                active_members = checked_inc(active_members)?;
                target_records_count = checked_add(target_records_count, member.final_targets().len())?;
                for target in member.final_targets() {
                    if matches!(target.selector_kind(), SelectorKind::Literal | SelectorKind::Path) {
                        let selector = target.selector().ok_or(CompilerError::PrivateInvariant {
                            kind: CompilerInvariant::InvalidByteRange,
                        })?;
                        target_selector_bytes = checked_add(target_selector_bytes, selector.text().len())?;
                    }
                    if target.regex_body().is_some() {
                        target_identity_slots = target_identity_slots.max(
                            target
                                .location()
                                .node_index()
                                .checked_add(1)
                                .ok_or(CompilerError::PreflightArithmetic)?,
                        );
                    }
                }
            }
        }
        let target_table_slots = canonical_table_slots(active_members)?;
        let transformation_table_slots = canonical_table_slots(active_members)?;
        let pattern_preflight = GroupPatternPreflight::inspect(&state, limits)?;
        let group_table_slots = canonical_table_slots(pattern_preflight.matcher_members)?;
        let mut compile_work = MatcherCompileWork {
            limits,
            validated_bytes: state.asset_templates.compilation_input_bytes,
            working_bytes: state.asset_templates.compiler_working_bytes,
            compiler_owned_bytes: state.compiler_owned_bytes,
            matcher_charged_bytes: state.matcher_charged_bytes,
            peak_bytes: state.peak_bytes,
            requirement_cursors: [0; 8],
        };
        compile_work.retain_working_records::<usize>(active_members)?;
        compile_work.retain_working_records::<usize>(target_identity_slots)?;
        compile_work.retain_working_records::<CandidateTargetPlan>(active_members)?;
        compile_work.retain_working_records::<CandidateTargetRecord>(target_records_count)?;
        compile_work.retain_working_bytes(target_selector_bytes)?;
        compile_work.retain_working_records::<CanonicalSlot>(target_table_slots)?;
        compile_work.retain_working_records::<CanonicalSlot>(transformation_table_slots)?;
        compile_work.retain_working_records::<CanonicalSlot>(group_table_slots)?;
        compile_work.retain_working_records::<CompiledMatcherGroupKey>(pattern_preflight.matcher_members)?;
        compile_work.retain_working_records::<CandidatePattern>(pattern_preflight.grouped_patterns)?;
        compile_work.retain_working_records::<MemberMatcherBinding>(pattern_preflight.matcher_members)?;
        compile_work.retain_working_records::<usize>(
            pattern_preflight
                .matcher_members
                .checked_mul(3)
                .ok_or(CompilerError::PreflightArithmetic)?,
        )?;
        compile_work.retain_working_records::<SemanticLocation>(pattern_preflight.matcher_members)?;
        compile_work.retain_working_records::<bool>(pattern_preflight.matcher_members)?;
        compile_work.retain_working_bytes(pattern_preflight.grouped_source_bytes)?;

        let mut active_member_indices = Vec::new();
        let mut target_regex_by_identity = Vec::new();
        let mut candidate_plans = Vec::new();
        let mut candidate_records = Vec::new();
        let mut candidate_bytes = Vec::new();
        let mut target_slots = Vec::new();
        let mut transformation_slots = Vec::new();
        let mut group_slots = Vec::new();
        let mut group_keys = Vec::new();
        let mut candidate_patterns = Vec::new();
        let mut member_bindings = Vec::new();
        let mut pattern_bytes = Vec::new();
        let mut group_heads = Vec::new();
        let mut group_tails = Vec::new();
        let mut group_counts = Vec::new();
        let mut group_locations = Vec::new();
        let mut group_has_regex = Vec::new();
        reserve_exact(&mut active_member_indices, active_members, "active member identities")?;
        reserve_exact(
            &mut target_regex_by_identity,
            target_identity_slots,
            "target regex identity map",
        )?;
        target_regex_by_identity.resize(target_identity_slots, usize::MAX);
        reserve_exact(&mut candidate_plans, active_members, "candidate target plans")?;
        reserve_exact(&mut candidate_records, target_records_count, "candidate target records")?;
        candidate_bytes
            .try_reserve_exact(target_selector_bytes)
            .map_err(|_| CompilerError::Allocation {
                operation: "candidate target bytes",
            })?;
        reserve_exact(&mut target_slots, target_table_slots, "target canonical table")?;
        target_slots.resize(target_table_slots, CanonicalSlot::EMPTY);
        reserve_exact(
            &mut transformation_slots,
            transformation_table_slots,
            "transformation canonical table",
        )?;
        transformation_slots.resize(transformation_table_slots, CanonicalSlot::EMPTY);
        reserve_exact(&mut group_slots, group_table_slots, "matcher group canonical table")?;
        group_slots.resize(group_table_slots, CanonicalSlot::EMPTY);
        reserve_exact(&mut group_keys, pattern_preflight.matcher_members, "matcher group keys")?;
        reserve_exact(
            &mut candidate_patterns,
            pattern_preflight.grouped_patterns,
            "candidate matcher patterns",
        )?;
        reserve_exact(
            &mut member_bindings,
            pattern_preflight.matcher_members,
            "matcher member bindings",
        )?;
        reserve_exact(
            &mut group_heads,
            pattern_preflight.matcher_members,
            "matcher group heads",
        )?;
        reserve_exact(
            &mut group_tails,
            pattern_preflight.matcher_members,
            "matcher group tails",
        )?;
        reserve_exact(
            &mut group_counts,
            pattern_preflight.matcher_members,
            "matcher group counts",
        )?;
        reserve_exact(
            &mut group_locations,
            pattern_preflight.matcher_members,
            "matcher group locations",
        )?;
        reserve_exact(
            &mut group_has_regex,
            pattern_preflight.matcher_members,
            "matcher group regex flags",
        )?;
        pattern_bytes
            .try_reserve_exact(pattern_preflight.grouped_source_bytes)
            .map_err(|_| CompilerError::Allocation {
                operation: "candidate matcher source bytes",
            })?;
        for target in &state.target_regexes {
            let identity = target.identity.node_index();
            let slot = target_regex_by_identity
                .get_mut(identity)
                .ok_or(CompilerError::PrivateInvariant {
                    kind: CompilerInvariant::MissingTargetRegex,
                })?;
            *slot = target.program;
        }

        let mut member_index = 0_usize;
        for unit in state.asset_templates.validated.rule_units() {
            for member in unit.members() {
                if unit.is_active() {
                    active_member_indices.push(member_index);
                    let record_start = candidate_records.len();
                    for target in member.final_targets() {
                        let selector = match target.selector_kind() {
                            SelectorKind::None => CandidateTargetSelector::None,
                            SelectorKind::Literal | SelectorKind::Path => {
                                let selector = target.selector().ok_or(CompilerError::PrivateInvariant {
                                    kind: CompilerInvariant::InvalidByteRange,
                                })?;
                                let start = candidate_bytes.len();
                                candidate_bytes.extend_from_slice(selector.text().as_bytes());
                                let range = ByteRange {
                                    start,
                                    len: selector.text().len(),
                                };
                                if target.selector_kind() == SelectorKind::Literal {
                                    CandidateTargetSelector::Literal(range)
                                } else {
                                    CandidateTargetSelector::Path(range)
                                }
                            }
                            SelectorKind::Regex(style) => {
                                let identity = target.location().node_index();
                                let program =
                                    *target_regex_by_identity
                                        .get(identity)
                                        .ok_or(CompilerError::PrivateInvariant {
                                            kind: CompilerInvariant::MissingTargetRegex,
                                        })?;
                                if program == usize::MAX {
                                    return Err(CompilerError::PrivateInvariant {
                                        kind: CompilerInvariant::MissingTargetRegex,
                                    });
                                }
                                CandidateTargetSelector::Regex { style, program }
                            }
                        };
                        candidate_records.push(CandidateTargetRecord {
                            collection: target.collection(),
                            modifier: target.modifier(),
                            selector,
                        });
                    }
                    candidate_plans.push(CandidateTargetPlan {
                        record_start,
                        record_count: candidate_records.len() - record_start,
                    });
                }
                member_index = checked_inc(member_index)?;
            }
        }
        if candidate_records.len() != target_records_count
            || candidate_bytes.len() != target_selector_bytes
            || candidate_plans.len() != active_members
        {
            return Err(CompilerError::PrivateInvariant {
                kind: CompilerInvariant::InvalidByteRange,
            });
        }

        let mut target_bytes = Vec::new();
        let mut target_records = Vec::new();
        let mut target_plans = Vec::new();
        let mut canonical_transformations = Vec::new();
        let mut transformation_plans = Vec::new();
        compile_work.reserve_canonical(&mut target_bytes, target_selector_bytes, "canonical target bytes")?;
        compile_work.reserve_canonical(&mut target_records, target_records_count, "canonical target records")?;
        compile_work.reserve_canonical(&mut target_plans, active_members, "canonical target plans")?;
        compile_work.reserve_canonical(
            &mut canonical_transformations,
            state.asset_templates.transformations.len(),
            "canonical transformations",
        )?;
        compile_work.reserve_canonical(
            &mut transformation_plans,
            active_members,
            "canonical transformation plans",
        )?;
        let mut canonical_work = std::mem::take(&mut state.canonical_work);
        for (ordinal, member_index) in active_member_indices.iter().copied().enumerate() {
            let target_candidate = TargetPlanRef {
                plan: candidate_plans[ordinal],
                records: &candidate_records,
                bytes: &candidate_bytes,
            };
            let target_hash = hash_target_plan(target_candidate, &mut canonical_work, limits)?;
            let target_plan = insert_target_plan(
                target_candidate,
                target_hash,
                &mut target_slots,
                &mut target_records,
                &mut target_bytes,
                &mut target_plans,
                &mut canonical_work,
                &mut compile_work,
            )?;
            let member = state.asset_templates.members[member_index];
            let transformation_end = member
                .transformation_start
                .checked_add(member.transformation_count)
                .ok_or(CompilerError::PreflightArithmetic)?;
            let transformations = state
                .asset_templates
                .transformations
                .get(member.transformation_start..transformation_end)
                .ok_or(CompilerError::PrivateInvariant {
                    kind: CompilerInvariant::InvalidByteRange,
                })?;
            let transformation_hash = hash_transformations(transformations, &mut canonical_work, limits)?;
            let transformation_plan = insert_transformation_plan(
                transformations,
                transformation_hash,
                &mut transformation_slots,
                &mut canonical_transformations,
                &mut transformation_plans,
                &mut canonical_work,
                &mut compile_work,
            )?;
            state.asset_templates.members[member_index].target_plan = Some(target_plan);
            state.asset_templates.members[member_index].transformation_plan = Some(transformation_plan);
        }
        let old_transformation_bytes = state
            .asset_templates
            .transformations
            .len()
            .checked_mul(size_of::<TransformationKind>())
            .ok_or(CompilerError::PreflightArithmetic)?;
        compile_work.drop_compiler_bytes(old_transformation_bytes)?;
        state.asset_templates.transformations.clear();

        let mut capture_phrases = Vec::new();
        reserve_exact(
            &mut capture_phrases,
            pattern_preflight.active_capture_phrase_programs,
            "capture phrase programs",
        )?;
        let mut asset_cursor = 0_usize;
        let mut source_member_index = 0_usize;
        for (unit_index, unit) in state.asset_templates.validated.rule_units().enumerate() {
            if unit.is_compatibility_excluded() {
                source_member_index = checked_add(source_member_index, unit.members().len())?;
                continue;
            }
            for member in unit.members() {
                let Some(operator) = member.operator() else {
                    source_member_index = checked_inc(source_member_index)?;
                    continue;
                };
                let capture = member.rule().fold().capture_action().is_some();
                let is_phrase = matches!(
                    operator.kind(),
                    super::super::OperatorKind::Pm | super::super::OperatorKind::PmFromFile
                );
                if is_phrase {
                    if unit.is_active() && !capture {
                        let key = member_group_key(
                            state.asset_templates.members[source_member_index],
                            member.rule().fold().multi_match_action().is_some(),
                        )?;
                        let hash = hash_group_key(key, &mut canonical_work, limits)?;
                        let group = insert_group_key(
                            key,
                            hash,
                            &mut group_slots,
                            &mut group_keys,
                            &mut canonical_work,
                            limits,
                        )?;
                        while group_heads.len() < group_keys.len() {
                            group_heads.push(usize::MAX);
                            group_tails.push(usize::MAX);
                            group_counts.push(0);
                            group_locations.push(operator.location());
                            group_has_regex.push(false);
                        }
                        let pattern_start = group_counts[group];
                        let pattern_before = candidate_patterns.len();
                        let count = visit_phrase_operand(
                            operator.operand(),
                            &state.asset_templates,
                            &mut asset_cursor,
                            |phrase, local_ordinal| {
                                append_candidate_pattern(
                                    group,
                                    CandidatePatternKind::Phrase,
                                    phrase,
                                    CompiledPatternMapping {
                                        unit: unit_index,
                                        member: source_member_index,
                                        local_ordinal,
                                    },
                                    &mut pattern_bytes,
                                    &mut candidate_patterns,
                                    &mut group_heads,
                                    &mut group_tails,
                                    &mut group_counts,
                                )
                            },
                        )?;
                        if candidate_patterns.len() - pattern_before != count {
                            return Err(CompilerError::PrivateInvariant {
                                kind: CompilerInvariant::InvalidByteRange,
                            });
                        }
                        member_bindings.push(MemberMatcherBinding {
                            member: source_member_index,
                            group,
                            pattern_start,
                            pattern_count: count,
                            kind: operator.kind(),
                            negated: operator.negated(),
                        });
                    } else {
                        let program = compile_phrase_operand_program(
                            operator.operand(),
                            &state.asset_templates,
                            &mut asset_cursor,
                            operator.location(),
                            unit.is_active(),
                            &mut compile_work,
                        )?;
                        if unit.is_active() {
                            compile_work.retain_compiler_records::<CompiledCapturePhraseProgram>(1)?;
                            let id = capture_phrases.len();
                            capture_phrases.push(program);
                            state.operator_plans[source_member_index] = Some(CompiledOperatorPlan {
                                kind: operator.kind(),
                                negated: operator.negated(),
                                operand: CompiledOperatorOperand::CapturePhrase(id),
                            });
                        }
                    }
                    discharge_requirement(
                        &mut state.asset_templates.requirements,
                        &mut compile_work.requirement_cursors,
                        CompileRequirement::PhraseMatcherCompilation,
                        operator.location(),
                    )?;
                } else if operator.kind() == super::super::OperatorKind::Rx && unit.is_active() && !capture {
                    let super::super::OperatorOperandView::Regex(source) = operator.operand() else {
                        return Err(CompilerError::PrivateInvariant {
                            kind: CompilerInvariant::UnexpectedActionOperand,
                        });
                    };
                    let key = member_group_key(
                        state.asset_templates.members[source_member_index],
                        member.rule().fold().multi_match_action().is_some(),
                    )?;
                    let hash = hash_group_key(key, &mut canonical_work, limits)?;
                    let group = insert_group_key(
                        key,
                        hash,
                        &mut group_slots,
                        &mut group_keys,
                        &mut canonical_work,
                        limits,
                    )?;
                    while group_heads.len() < group_keys.len() {
                        group_heads.push(usize::MAX);
                        group_tails.push(usize::MAX);
                        group_counts.push(0);
                        group_locations.push(operator.location());
                        group_has_regex.push(false);
                    }
                    group_has_regex[group] = true;
                    let pattern_start = group_counts[group];
                    append_candidate_pattern(
                        group,
                        CandidatePatternKind::Regex,
                        source.text(),
                        CompiledPatternMapping {
                            unit: unit_index,
                            member: source_member_index,
                            local_ordinal: 0,
                        },
                        &mut pattern_bytes,
                        &mut candidate_patterns,
                        &mut group_heads,
                        &mut group_tails,
                        &mut group_counts,
                    )?;
                    member_bindings.push(MemberMatcherBinding {
                        member: source_member_index,
                        group,
                        pattern_start,
                        pattern_count: 1,
                        kind: operator.kind(),
                        negated: operator.negated(),
                    });
                    discharge_requirement(
                        &mut state.asset_templates.requirements,
                        &mut compile_work.requirement_cursors,
                        CompileRequirement::RegexCompilation,
                        operator.location(),
                    )?;
                }
                source_member_index = checked_inc(source_member_index)?;
            }
        }
        if asset_cursor != state.asset_templates.assets.len()
            || candidate_patterns.len() != pattern_preflight.grouped_patterns
            || pattern_bytes.len() != pattern_preflight.grouped_source_bytes
            || member_bindings.len() != pattern_preflight.matcher_members
            || capture_phrases.len() != pattern_preflight.active_capture_phrase_programs
        {
            return Err(CompilerError::PrivateInvariant {
                kind: CompilerInvariant::InvalidByteRange,
            });
        }

        // Selector identity records can share a program. Programs are inserted as
        // a contiguous prefix before any operator-only boolean programs.
        let target_boolean_programs = state
            .target_regexes
            .iter()
            .map(|selector| selector.program)
            .max()
            .map(checked_inc)
            .transpose()?
            .unwrap_or(0);
        let final_regex_programs = target_boolean_programs
            .checked_add(state.capture_regexes.len())
            .and_then(|count| count.checked_add(group_has_regex.iter().filter(|present| **present).count()))
            .ok_or(CompilerError::PreflightArithmetic)?;
        if final_regex_programs > limits.max_regex_programs {
            return Err(CompilerError::RegexProgramLimit {
                actual: final_regex_programs,
                limit: limits.max_regex_programs,
            });
        }

        compile_work.retain_compiler_records::<CompiledMatcherGroup>(group_keys.len())?;
        let mapping_bytes = candidate_patterns
            .len()
            .checked_mul(size_of::<CompiledPatternMapping>())
            .ok_or(CompilerError::PreflightArithmetic)?;
        compile_work.retain_matcher(mapping_bytes)?;
        let mut groups = Vec::new();
        let mut pattern_mappings = Vec::new();
        reserve_exact(&mut groups, group_keys.len(), "compiled matcher groups")?;
        reserve_exact(
            &mut pattern_mappings,
            candidate_patterns.len(),
            "matcher pattern mappings",
        )?;
        for group in 0..group_keys.len() {
            let count = group_counts[group];
            compile_work.retain_working_records::<PatternSpec<'_>>(count)?;
            let mut patterns = Vec::new();
            reserve_exact(&mut patterns, count, "group matcher pattern references")?;
            let mapping_start = pattern_mappings.len();
            let mut counts = MatcherPatternCounts::default();
            let mut bytes = MatcherPatternBytes::default();
            let mut next = group_heads[group];
            while next != usize::MAX {
                let candidate = *candidate_patterns.get(next).ok_or(CompilerError::PrivateInvariant {
                    kind: CompilerInvariant::InvalidByteRange,
                })?;
                if candidate.group != group {
                    return Err(CompilerError::PrivateInvariant {
                        kind: CompilerInvariant::InvalidByteRange,
                    });
                }
                let source = std::str::from_utf8(byte_range(&pattern_bytes, candidate.bytes)?).map_err(|_| {
                    CompilerError::PrivateInvariant {
                        kind: CompilerInvariant::InvalidByteRange,
                    }
                })?;
                match candidate.kind {
                    CandidatePatternKind::Phrase => {
                        counts.insensitive_phrases = checked_inc(counts.insensitive_phrases)?;
                        bytes.insensitive_phrases = checked_add(bytes.insensitive_phrases, source.len())?;
                        patterns.push(PatternSpec::Phrase {
                            value: source,
                            ascii_case_insensitive: true,
                        });
                    }
                    CandidatePatternKind::Regex => {
                        counts.regexes = checked_inc(counts.regexes)?;
                        bytes.regexes = checked_add(bytes.regexes, source.len())?;
                        patterns.push(PatternSpec::Regex(source));
                    }
                }
                pattern_mappings.push(candidate.mapping);
                next = candidate.next;
            }
            if patterns.len() != count {
                return Err(CompilerError::PrivateInvariant {
                    kind: CompilerInvariant::InvalidByteRange,
                });
            }
            let matcher = compile_work.compile_group_matcher(&patterns, counts, bytes, group_locations[group])?;
            compile_work.drop_working_records::<PatternSpec<'_>>(count);
            groups.push(CompiledMatcherGroup {
                key: group_keys[group],
                matcher,
                mapping_start,
                mapping_count: count,
            });
        }
        for binding in &member_bindings {
            state.operator_plans[binding.member] = Some(CompiledOperatorPlan {
                kind: binding.kind,
                negated: binding.negated,
                operand: CompiledOperatorOperand::Matcher {
                    group: binding.group,
                    pattern_start: binding.pattern_start,
                    pattern_count: binding.pattern_count,
                },
            });
        }
        let dropped_boolean_bytes = state
            .boolean_regexes
            .get(target_boolean_programs..)
            .ok_or(CompilerError::PrivateInvariant {
                kind: CompilerInvariant::InvalidByteRange,
            })?
            .iter()
            .try_fold(0_usize, |total, program| total.checked_add(program.memory_usage()))
            .ok_or(CompilerError::PreflightArithmetic)?;
        state.boolean_regexes.truncate(target_boolean_programs);
        compile_work.drop_matcher(dropped_boolean_bytes)?;
        if state
            .asset_templates
            .requirements
            .iter()
            .any(|requirement| !requirement.discharged)
        {
            return Err(CompilerError::PrivateInvariant {
                kind: CompilerInvariant::MissingRequirement,
            });
        }

        compile_work.drop_working_records::<bool>(pattern_preflight.matcher_members);
        compile_work.drop_working_records::<SemanticLocation>(pattern_preflight.matcher_members);
        compile_work.drop_working_records::<usize>(
            pattern_preflight
                .matcher_members
                .checked_mul(3)
                .ok_or(CompilerError::PreflightArithmetic)?,
        );
        compile_work.drop_working_bytes(pattern_preflight.grouped_source_bytes);
        compile_work.drop_working_records::<MemberMatcherBinding>(pattern_preflight.matcher_members);
        compile_work.drop_working_records::<CandidatePattern>(pattern_preflight.grouped_patterns);
        compile_work.drop_working_records::<CompiledMatcherGroupKey>(pattern_preflight.matcher_members);
        compile_work.drop_working_records::<CanonicalSlot>(group_table_slots);
        compile_work.drop_working_records::<CanonicalSlot>(transformation_table_slots);
        compile_work.drop_working_records::<CanonicalSlot>(target_table_slots);
        compile_work.drop_working_bytes(target_selector_bytes);
        compile_work.drop_working_records::<CandidateTargetRecord>(target_records_count);
        compile_work.drop_working_records::<CandidateTargetPlan>(active_members);
        compile_work.drop_working_records::<usize>(target_identity_slots);
        compile_work.drop_working_records::<usize>(active_members);

        Ok(Self {
            pattern_preflight,
            matchers: state,
            canonical_work,
            target_bytes,
            target_records,
            target_plans,
            canonical_transformations,
            transformation_plans,
            groups,
            pattern_mappings,
            capture_phrases,
            compiler_owned_bytes: compile_work.compiler_owned_bytes,
            matcher_charged_bytes: compile_work.matcher_charged_bytes,
            peak_bytes: compile_work.peak_bytes,
        })
    }
}
