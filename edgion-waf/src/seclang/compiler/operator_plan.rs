use super::*;

#[derive(Clone, Copy)]
pub(super) struct MatcherCompileWork {
    pub(super) limits: CompilerLimits,
    pub(super) validated_bytes: usize,
    pub(super) working_bytes: usize,
    pub(super) compiler_owned_bytes: usize,
    pub(super) matcher_charged_bytes: usize,
    pub(super) peak_bytes: usize,
    pub(super) requirement_cursors: [usize; 8],
}

pub(super) fn install_optional_regex_hint(
    program: &mut CompiledRegexProgram,
    program_cap: usize,
    work: &mut MatcherCompileWork,
    canonical: &mut CanonicalWork,
) -> Result<(), CompilerError> {
    let base = work.matcher_peak_base()?;
    let generation_bytes = work
        .compiler_owned_bytes
        .checked_add(work.matcher_charged_bytes)
        .ok_or(CompilerError::PreflightArithmetic)?;
    let owned = work
        .limits
        .max_compiled_matcher_bytes
        .saturating_sub(work.matcher_charged_bytes)
        .min(work.limits.max_generation_bytes.saturating_sub(generation_bytes))
        .min(program_cap.saturating_sub(program.memory_usage().max(program.conservative_program_memory_bound())));
    let mut budget = crate::matcher::RegexGuardBudget {
        owned,
        peak: work.limits.max_compilation_peak_bytes.saturating_sub(base),
        work: work
            .limits
            .max_group_key_record_comparisons
            .saturating_sub(canonical.record_comparisons),
    };
    let (simple, installed) = program.lower_simple(budget).map_err(|error| match error {
        RegexProgramError::Arithmetic => CompilerError::PreflightArithmetic,
        _ => CompilerError::PrivateInvariant {
            kind: CompilerInvariant::InvalidByteRange,
        },
    })?;
    canonical.charge_records(simple.work, work.limits)?;
    if simple.peak != 0 {
        work.record_matcher_peak(simple.peak)?;
    }
    if installed {
        return Ok(());
    }
    budget.work = budget
        .work
        .checked_sub(simple.work)
        .ok_or(CompilerError::PreflightArithmetic)?;
    let outcome = program.add_literal_guard(budget).map_err(|error| match error {
        RegexProgramError::Arithmetic => CompilerError::PreflightArithmetic,
        _ => CompilerError::PrivateInvariant {
            kind: CompilerInvariant::InvalidByteRange,
        },
    })?;
    canonical.charge_records(outcome.work, work.limits)?;
    if outcome.peak != 0 {
        work.record_matcher_peak(outcome.peak)?;
    }
    work.retain_matcher(outcome.owned)?;
    Ok(())
}

impl MatcherCompileWork {
    // Canonical identity counts are independent of retained backing capacity.
    // Admit growth and old/new overlap before allocation, then reconcile excess.
    pub(super) fn reserve_canonical<T>(
        &mut self,
        values: &mut Vec<T>,
        additional: usize,
        operation: &'static str,
    ) -> Result<(), CompilerError> {
        let required = checked_add(values.len(), additional)?;
        let old = values.capacity();
        if required <= old {
            return Ok(());
        }
        let overlap = old
            .checked_mul(size_of::<T>())
            .ok_or(CompilerError::PreflightArithmetic)?;
        self.retain_compiler_records::<T>(required - old)?;
        self.record_matcher_peak(overlap)?;
        reserve_exact(values, additional, operation)?;
        let excess = values
            .capacity()
            .checked_sub(required)
            .ok_or(CompilerError::PrivateInvariant {
                kind: CompilerInvariant::InvalidByteRange,
            })?;
        self.retain_compiler_records::<T>(excess)?;
        self.record_matcher_peak(overlap)
    }

    fn matcher_peak_base(&self) -> Result<usize, CompilerError> {
        self.validated_bytes
            .checked_add(self.compiler_owned_bytes)
            .and_then(|bytes| bytes.checked_add(self.working_bytes))
            .and_then(|bytes| bytes.checked_add(self.matcher_charged_bytes))
            .ok_or(CompilerError::PreflightArithmetic)
    }

    pub(super) fn record_matcher_peak(&mut self, additional: usize) -> Result<(), CompilerError> {
        let snapshot = self
            .matcher_peak_base()?
            .checked_add(additional)
            .ok_or(CompilerError::PreflightArithmetic)?;
        self.peak_bytes = self.peak_bytes.max(snapshot);
        if snapshot > self.limits.max_compilation_peak_bytes {
            return Err(CompilerError::PeakByteLimit {
                limit: self.limits.max_compilation_peak_bytes,
            });
        }
        Ok(())
    }

    pub(super) fn precheck_matcher_projection(
        &mut self,
        projection: MatcherBuildProjection,
    ) -> Result<(), CompilerError> {
        let peak = projection
            .conservative_peak_bytes()
            .map_err(|_| CompilerError::PreflightArithmetic)?;
        self.record_matcher_peak(peak)
    }

    pub(super) fn compile_capture_phrase(
        &mut self,
        patterns: &[&str],
        source_bytes: usize,
        location: SemanticLocation,
        retain: bool,
    ) -> Result<CompiledCapturePhraseProgram, CompilerError> {
        let projection = MatcherBuildProjection::for_counts(
            MatcherPatternCounts {
                insensitive_phrases: patterns.len(),
                ..MatcherPatternCounts::default()
            },
            MatcherPatternBytes {
                insensitive_phrases: source_bytes,
                ..MatcherPatternBytes::default()
            },
            self.limits.matcher,
        )
        .map_err(|error| map_phrase_matcher_error(error, location))?;
        self.precheck_matcher_projection(projection)?;
        let program = CompiledCapturePhraseProgram::compile(patterns, self.limits.matcher)
            .map_err(|error| map_phrase_matcher_error(error, location))?;
        self.record_matcher_peak(program.estimated_peak_build_memory_usage())?;
        if retain {
            self.retain_matcher(program.memory_usage())?;
        }
        Ok(program)
    }

    pub(super) fn compile_group_matcher(
        &mut self,
        patterns: &[PatternSpec<'_>],
        counts: MatcherPatternCounts,
        bytes: MatcherPatternBytes,
        location: SemanticLocation,
    ) -> Result<CompiledMatcher, CompilerError> {
        let projection = MatcherBuildProjection::for_counts(counts, bytes, self.limits.matcher)
            .map_err(|error| map_phrase_matcher_error(error, location))?;
        self.precheck_matcher_projection(projection)?;
        let matcher = CompiledMatcher::compile_pcre_bytes(patterns, self.limits.matcher)
            .map_err(|error| map_phrase_matcher_error(error, location))?;
        self.record_matcher_peak(matcher.estimated_peak_build_memory_usage())?;
        self.retain_matcher(matcher.memory_usage())?;
        Ok(matcher)
    }

    pub(super) fn compile_boolean_regex(
        &mut self,
        source: &str,
        location: SemanticLocation,
        active: bool,
    ) -> Result<CompiledRegexProgram, CompilerError> {
        let (base, budget) = self.regex_peak_budget()?;
        let program = CompiledRegexProgram::compile_one_pcre_bytes_with_peak_budget(source, self.limits.regex, budget)
            .map_err(|error| self.map_regex_error(location, error))?;
        self.record_regex_peak(base, program.estimated_peak_build_memory_usage())?;
        if active {
            self.retain_matcher(program.memory_usage())?;
        }
        Ok(program)
    }

    pub(super) fn compile_ascii_case_insensitive_boolean_regex(
        &mut self,
        source: &str,
        location: SemanticLocation,
        active: bool,
    ) -> Result<CompiledRegexProgram, CompilerError> {
        let (base, budget) = self.regex_peak_budget()?;
        let program = CompiledRegexProgram::compile_one_ascii_case_insensitive_with_peak_budget(
            source,
            self.limits.regex,
            budget,
        )
        .map_err(|error| self.map_regex_error(location, error))?;
        self.record_regex_peak(base, program.estimated_peak_build_memory_usage())?;
        if active {
            self.retain_matcher(program.memory_usage())?;
        }
        Ok(program)
    }

    pub(super) fn compile_capture_regex(
        &mut self,
        source: &str,
        location: SemanticLocation,
        active: bool,
    ) -> Result<CompiledCaptureRegexProgram, CompilerError> {
        let (base, budget) = self.regex_peak_budget()?;
        let program =
            CompiledCaptureRegexProgram::compile_one_pcre_bytes_with_peak_budget(source, self.limits.regex, budget)
                .map_err(|error| self.map_regex_error(location, error))?;
        self.record_regex_peak(base, program.estimated_peak_build_memory_usage())?;
        if active {
            self.retain_matcher(program.memory_usage())?;
        }
        Ok(program)
    }

    pub(super) fn regex_peak_budget(&self) -> Result<(usize, usize), CompilerError> {
        let base = self
            .validated_bytes
            .checked_add(self.compiler_owned_bytes)
            .and_then(|bytes| bytes.checked_add(self.working_bytes))
            .and_then(|bytes| bytes.checked_add(self.matcher_charged_bytes))
            .and_then(|bytes| bytes.checked_add(size_of::<&str>()))
            .ok_or(CompilerError::PreflightArithmetic)?;
        let budget = self
            .limits
            .max_compilation_peak_bytes
            .checked_sub(base)
            .ok_or(CompilerError::PeakByteLimit {
                limit: self.limits.max_compilation_peak_bytes,
            })?;
        Ok((base, budget))
    }

    fn record_regex_peak(&mut self, base: usize, regex_peak: usize) -> Result<(), CompilerError> {
        let snapshot = base.checked_add(regex_peak).ok_or(CompilerError::PreflightArithmetic)?;
        self.peak_bytes = self.peak_bytes.max(snapshot);
        if snapshot > self.limits.max_compilation_peak_bytes {
            return Err(CompilerError::PeakByteLimit {
                limit: self.limits.max_compilation_peak_bytes,
            });
        }
        Ok(())
    }

    fn map_regex_error(&self, location: SemanticLocation, error: RegexProgramError) -> CompilerError {
        if error == RegexProgramError::SyntaxPeakLimit {
            CompilerError::PeakByteLimit {
                limit: self.limits.max_compilation_peak_bytes,
            }
        } else {
            CompilerError::RegexProgram { location, error }
        }
    }

    pub(super) fn retain_matcher(&mut self, bytes: usize) -> Result<(), CompilerError> {
        let retained = self
            .matcher_charged_bytes
            .checked_add(bytes)
            .ok_or(CompilerError::PreflightArithmetic)?;
        if retained > self.limits.max_compiled_matcher_bytes {
            return Err(CompilerError::CompiledMatcherByteLimit {
                limit: self.limits.max_compiled_matcher_bytes,
            });
        }
        let generation = self
            .compiler_owned_bytes
            .checked_add(retained)
            .ok_or(CompilerError::PreflightArithmetic)?;
        if generation > self.limits.max_generation_bytes {
            return Err(CompilerError::GenerationByteLimit {
                limit: self.limits.max_generation_bytes,
            });
        }
        self.matcher_charged_bytes = retained;
        Ok(())
    }

    pub(super) fn drop_matcher(&mut self, bytes: usize) -> Result<(), CompilerError> {
        self.matcher_charged_bytes =
            self.matcher_charged_bytes
                .checked_sub(bytes)
                .ok_or(CompilerError::PrivateInvariant {
                    kind: CompilerInvariant::InvalidByteRange,
                })?;
        Ok(())
    }

    pub(super) fn retain_compiler_records<T>(&mut self, count: usize) -> Result<(), CompilerError> {
        let bytes = count
            .checked_mul(size_of::<T>())
            .ok_or(CompilerError::PreflightArithmetic)?;
        self.retain_compiler_bytes(bytes)
    }

    fn retain_compiler_bytes(&mut self, bytes: usize) -> Result<(), CompilerError> {
        let owned = self
            .compiler_owned_bytes
            .checked_add(bytes)
            .ok_or(CompilerError::PreflightArithmetic)?;
        if owned > self.limits.max_compiler_owned_non_matcher_bytes {
            return Err(CompilerError::OwnedByteLimit {
                limit: self.limits.max_compiler_owned_non_matcher_bytes,
            });
        }
        let generation = owned
            .checked_add(self.matcher_charged_bytes)
            .ok_or(CompilerError::PreflightArithmetic)?;
        if generation > self.limits.max_generation_bytes {
            return Err(CompilerError::GenerationByteLimit {
                limit: self.limits.max_generation_bytes,
            });
        }
        let snapshot = self
            .validated_bytes
            .checked_add(owned)
            .and_then(|bytes| bytes.checked_add(self.working_bytes))
            .and_then(|bytes| bytes.checked_add(self.matcher_charged_bytes))
            .ok_or(CompilerError::PreflightArithmetic)?;
        self.peak_bytes = self.peak_bytes.max(snapshot);
        if snapshot > self.limits.max_compilation_peak_bytes {
            return Err(CompilerError::PeakByteLimit {
                limit: self.limits.max_compilation_peak_bytes,
            });
        }
        self.compiler_owned_bytes = owned;
        Ok(())
    }

    pub(super) fn drop_compiler_bytes(&mut self, bytes: usize) -> Result<(), CompilerError> {
        self.compiler_owned_bytes =
            self.compiler_owned_bytes
                .checked_sub(bytes)
                .ok_or(CompilerError::PrivateInvariant {
                    kind: CompilerInvariant::InvalidByteRange,
                })?;
        Ok(())
    }

    pub(super) fn retain_working_records<T>(&mut self, count: usize) -> Result<(), CompilerError> {
        let bytes = count
            .checked_mul(size_of::<T>())
            .ok_or(CompilerError::PreflightArithmetic)?;
        self.retain_working_bytes(bytes)
    }

    pub(super) fn retain_working_bytes(&mut self, bytes: usize) -> Result<(), CompilerError> {
        self.working_bytes = self
            .working_bytes
            .checked_add(bytes)
            .ok_or(CompilerError::PreflightArithmetic)?;
        let snapshot = self
            .validated_bytes
            .checked_add(self.compiler_owned_bytes)
            .and_then(|value| value.checked_add(self.working_bytes))
            .and_then(|value| value.checked_add(self.matcher_charged_bytes))
            .ok_or(CompilerError::PreflightArithmetic)?;
        self.peak_bytes = self.peak_bytes.max(snapshot);
        if snapshot > self.limits.max_compilation_peak_bytes {
            return Err(CompilerError::PeakByteLimit {
                limit: self.limits.max_compilation_peak_bytes,
            });
        }
        Ok(())
    }

    pub(super) fn drop_working_records<T>(&mut self, count: usize) {
        let bytes = count
            .checked_mul(size_of::<T>())
            .expect("bounded working record charge remains representable");
        self.working_bytes = self
            .working_bytes
            .checked_sub(bytes)
            .expect("working records were retained before drop");
    }

    pub(super) fn drop_working_bytes(&mut self, bytes: usize) {
        self.working_bytes = self
            .working_bytes
            .checked_sub(bytes)
            .expect("working bytes were retained before drop");
    }
}

// Independent inputs: the only caller builds MatcherState from these output
// tables after its loop (which borrows state.validated), so no method fits.
#[allow(clippy::too_many_arguments)]
pub(super) fn compile_operator_plan<'view, 'source>(
    operator: super::super::SemanticOperatorView<'view, 'source>,
    member: super::super::ValidatedRuleNodeView<'view, 'source>,
    operator_template: Option<usize>,
    ip_networks: &mut Vec<CompiledIpNetwork>,
    boolean_regexes: &mut Vec<CompiledRegexProgram>,
    capture_regexes: &mut Vec<CompiledCaptureRegexProgram>,
    capture_interner: &mut RegexProgramInterner<'view>,
    canonical_work: &mut CanonicalWork,
    requirements: &mut [RequirementState],
    work: &mut MatcherCompileWork,
    active: bool,
) -> Result<Option<CompiledOperatorPlan>, CompilerError> {
    use super::super::OperatorKind;

    let kind = operator.kind();
    if matches!(kind, OperatorKind::Pm | OperatorKind::PmFromFile) {
        return Ok(None);
    }
    if matches!(kind, OperatorKind::DetectSqli | OperatorKind::DetectXss) && !active {
        return Ok(None);
    }
    let operand = match (kind, operator.operand()) {
        (
            OperatorKind::BeginsWith
            | OperatorKind::Contains
            | OperatorKind::EndsWith
            | OperatorKind::Streq
            | OperatorKind::Within,
            super::super::OperatorOperandView::StringTemplate(_),
        ) => {
            let relation = match kind {
                OperatorKind::BeginsWith => StringRelation::BeginsWith,
                OperatorKind::Contains => StringRelation::Contains,
                OperatorKind::EndsWith => StringRelation::EndsWith,
                OperatorKind::Streq => StringRelation::Equals,
                OperatorKind::Within => StringRelation::Within,
                _ => unreachable!("the enclosing arm admits only the five string relation operators"),
            };
            CompiledOperatorOperand::String {
                relation,
                template: operator_template.ok_or(CompilerError::PrivateInvariant {
                    kind: CompilerInvariant::UnexpectedActionOperand,
                })?,
            }
        }
        (
            OperatorKind::Eq | OperatorKind::Ge | OperatorKind::Gt | OperatorKind::Lt,
            super::super::OperatorOperandView::IntegerTemplate { constant, .. },
        ) => {
            let relation = match kind {
                OperatorKind::Eq => IntegerRelation::Equal,
                OperatorKind::Ge => IntegerRelation::GreaterOrEqual,
                OperatorKind::Gt => IntegerRelation::Greater,
                OperatorKind::Lt => IntegerRelation::Less,
                _ => unreachable!("the enclosing arm admits only the Eq, Ge, Gt and Lt operators"),
            };
            let operand = match constant {
                Some(value) => CompiledIntegerOperand::Static(value),
                None => CompiledIntegerOperand::Template(operator_template.ok_or(CompilerError::PrivateInvariant {
                    kind: CompilerInvariant::UnexpectedActionOperand,
                })?),
            };
            CompiledOperatorOperand::Integer { relation, operand }
        }
        (OperatorKind::IpMatch, super::super::OperatorOperandView::IpNetworks(networks)) => {
            let start = ip_networks.len();
            if active {
                for network in networks.networks() {
                    ip_networks.push(CompiledIpNetwork {
                        address: network.address(),
                        prefix: network.prefix(),
                    });
                }
            }
            CompiledOperatorOperand::IpNetworks {
                start,
                count: if active { ip_networks.len() - start } else { 0 },
            }
        }
        (OperatorKind::ValidateByteRange, super::super::OperatorOperandView::ByteSet(bytes)) => {
            CompiledOperatorOperand::ByteSet { words: bytes.words() }
        }
        (OperatorKind::ValidateUrlEncoding, super::super::OperatorOperandView::NoArgument) => {
            CompiledOperatorOperand::ValidateUrlEncoding
        }
        (OperatorKind::ValidateUtf8Encoding, super::super::OperatorOperandView::NoArgument) => {
            CompiledOperatorOperand::ValidateUtf8Encoding
        }
        (OperatorKind::UnconditionalMatch, super::super::OperatorOperandView::NoArgument) => {
            CompiledOperatorOperand::Unconditional
        }
        (OperatorKind::DetectSqli, super::super::OperatorOperandView::NoArgument) => {
            CompiledOperatorOperand::DetectSqli {
                capture: member.rule().fold().capture_action().is_some(),
            }
        }
        (OperatorKind::DetectXss, super::super::OperatorOperandView::NoArgument) => CompiledOperatorOperand::DetectXss,
        (OperatorKind::Rx, super::super::OperatorOperandView::Regex(regex)) => {
            let location = operator.location();
            let capture = member.rule().fold().capture_action().is_some();
            if active && !capture {
                return Ok(None);
            }
            let reference = if capture {
                if active {
                    let id = capture_interner.compile(regex.text(), location, capture_regexes, work, canonical_work)?;
                    RegexPlanRef::Capture(id)
                } else {
                    // Removed, nonexcluded rules still validate independently.
                    work.compile_capture_regex(regex.text(), location, false)?;
                    RegexPlanRef::Capture(usize::MAX)
                }
            } else {
                let program = work.compile_boolean_regex(regex.text(), location, active)?;
                if active {
                    let id = boolean_regexes.len();
                    boolean_regexes.push(program);
                    RegexPlanRef::Boolean(id)
                } else {
                    RegexPlanRef::Boolean(usize::MAX)
                }
            };
            discharge_requirement(
                requirements,
                &mut work.requirement_cursors,
                CompileRequirement::RegexCompilation,
                location,
            )?;
            CompiledOperatorOperand::Regex(reference)
        }
        _ => {
            return Err(CompilerError::PrivateInvariant {
                kind: CompilerInvariant::UnexpectedActionOperand,
            });
        }
    };
    if !active {
        return Ok(None);
    }
    Ok(Some(CompiledOperatorPlan {
        kind,
        negated: operator.negated(),
        operand,
    }))
}

pub(super) fn discharge_requirement(
    requirements: &mut [RequirementState],
    cursors: &mut [usize; 8],
    kind: CompileRequirement,
    location: SemanticLocation,
) -> Result<(), CompilerError> {
    let category = requirement_category(kind);
    let key = requirement_key(kind, location);
    let Ok(index) = requirements.binary_search_by_key(&key, |requirement| {
        requirement_key(requirement.kind, requirement.location)
    }) else {
        return Err(CompilerError::PrivateInvariant {
            kind: CompilerInvariant::MissingRequirement,
        });
    };
    let requirement = &mut requirements[index];
    if requirement.discharged {
        return Err(CompilerError::PrivateInvariant {
            kind: CompilerInvariant::DuplicateRequirement,
        });
    }
    requirement.discharged = true;
    cursors[category] = cursors[category].saturating_add(1);
    Ok(())
}

pub(super) fn verify_b_requirements(requirements: &[RequirementState]) -> Result<(), CompilerError> {
    for requirement in requirements {
        if !matches!(
            requirement.kind,
            CompileRequirement::PhraseMatcherCompilation | CompileRequirement::RegexCompilation
        ) && !requirement.discharged
        {
            return Err(CompilerError::PrivateInvariant {
                kind: CompilerInvariant::MissingRequirement,
            });
        }
    }
    Ok(())
}
