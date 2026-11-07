use super::*;

#[derive(Debug, Default)]
pub(super) struct KnownPreflight {
    members: usize,
    actions: usize,
    transformations: usize,
    templates: usize,
    segments: usize,
    macros: usize,
    setvars: usize,
    metadata_bytes: usize,
    asset_requests: usize,
    first_template_location: Option<SemanticLocation>,
    max_member_runtime_templates: usize,
    first_member_runtime_error: Option<(usize, DirectiveLocation)>,
}

impl KnownPreflight {
    pub(super) fn inspect(
        validated: &ValidatedSemanticDocument<'_>,
        limits: CompilerLimits,
    ) -> Result<Self, CompilerError> {
        let mut result = Self::default();
        for unit in validated.rule_units() {
            for member in unit.members() {
                result.members = checked_inc(result.members)?;
                if unit.is_compatibility_excluded() {
                    continue;
                }
                let mut runtime_templates = 0_usize;
                if let Some(operator) = member.operator() {
                    if let OperatorOperandView::ExternalAssets(external) = operator.operand() {
                        result.asset_requests = checked_add(result.asset_requests, external.assets().len())?;
                    }
                    if let Some(template) = operator.template() {
                        runtime_templates = checked_inc(runtime_templates)?;
                        result.add_template(
                            template.segments().map(|segment| (segment.text(), segment.kind())),
                            operator.location(),
                        )?;
                    }
                }
                for action in member.actions() {
                    result.actions = checked_inc(result.actions)?;
                    match action.operand() {
                        ActionOperandView::Template(template) => {
                            runtime_templates = checked_inc(runtime_templates)?;
                            result.add_template(
                                template.segments().map(|segment| (segment.text(), segment.kind())),
                                action.location(),
                            )?;
                        }
                        ActionOperandView::Literal(value) => {
                            result.metadata_bytes = checked_add(result.metadata_bytes, value.text().len())?;
                        }
                        ActionOperandView::SetVar(setvar) => {
                            runtime_templates = checked_inc(runtime_templates)?;
                            result.setvars = checked_inc(result.setvars)?;
                            result.add_template(
                                setvar.key().segments().map(|segment| (segment.text(), segment.kind())),
                                action.location(),
                            )?;
                            if let Some(value) = setvar.value() {
                                result.add_template(
                                    value.segments().map(|segment| (segment.text(), segment.kind())),
                                    action.location(),
                                )?;
                            }
                        }
                        ActionOperandView::Ctl(ctl) => {
                            let bytes = match ctl {
                                CtlView::AuditEngine(_)
                                | CtlView::ForceRequestBodyVariable(_)
                                | CtlView::RequestBodyProcessor(_) => 0,
                                CtlView::RuleRemoveByTag(tag) => tag.text().len(),
                                CtlView::RuleRemoveTargetByTag { tag, target } => tag
                                    .text()
                                    .len()
                                    .checked_add(
                                        target
                                            .regex_body()
                                            .or_else(|| target.selector())
                                            .map_or(0, |value| value.text().len()),
                                    )
                                    .ok_or(CompilerError::PreflightArithmetic)?,
                            };
                            result.metadata_bytes = checked_add(result.metadata_bytes, bytes)?;
                        }
                        ActionOperandView::NoArgument
                        | ActionOperandView::Local(_)
                        | ActionOperandView::Transformation(_)
                        | ActionOperandView::InitCol(_) => {}
                    }
                }
                result.transformations = checked_add(result.transformations, member.effective_transformations().len())?;
                let runtime_projection = runtime_templates.checked_mul(limits.max_template_runtime_bytes);
                let runtime_bytes = runtime_projection.unwrap_or(usize::MAX);
                if (runtime_projection.is_none() || runtime_bytes > limits.max_rule_runtime_expansion_bytes)
                    && result.first_member_runtime_error.is_none()
                {
                    result.first_member_runtime_error = Some((runtime_bytes, member.rule().location()));
                }
                result.max_member_runtime_templates = result.max_member_runtime_templates.max(runtime_templates);
            }
        }
        Ok(result)
    }

    fn add_template<'text, 'source>(
        &mut self,
        segments: impl Iterator<Item = (&'text str, TemplateSegmentKind<'text, 'source>)>,
        location: SemanticLocation,
    ) -> Result<(), CompilerError>
    where
        'source: 'text,
    {
        self.templates = checked_inc(self.templates)?;
        self.first_template_location.get_or_insert(location);
        for (text, kind) in segments {
            self.segments = checked_inc(self.segments)?;
            match kind {
                TemplateSegmentKind::Literal => {
                    self.metadata_bytes = checked_add(self.metadata_bytes, text.len())?;
                }
                TemplateSegmentKind::Expansion(expansion) => {
                    self.macros = checked_inc(self.macros)?;
                    self.metadata_bytes =
                        checked_add(self.metadata_bytes, expansion.key().map_or(0, |key| key.text().len()))?;
                }
            }
        }
        Ok(())
    }

    pub(super) fn validate(&self, limits: CompilerLimits) -> Result<(), CompilerError> {
        if self.actions > limits.max_action_plans {
            return Err(CompilerError::ActionLimit {
                actual: self.actions,
                limit: limits.max_action_plans,
            });
        }
        if self.asset_requests > limits.max_asset_requests {
            return Err(CompilerError::AssetRequestLimit {
                actual: self.asset_requests,
                limit: limits.max_asset_requests,
            });
        }
        if self.templates > limits.max_templates {
            return Err(CompilerError::TemplateLimit {
                limit: limits.max_templates,
                location: self
                    .first_template_location
                    .expect("nonzero compiled template count has a source location"),
            });
        }
        if self.segments > limits.max_template_segments {
            return Err(CompilerError::TemplateSegmentLimit {
                limit: limits.max_template_segments,
                location: self
                    .first_template_location
                    .expect("nonzero compiled segment count has a source location"),
            });
        }
        if let Some((actual, location)) = self.first_member_runtime_error {
            return Err(CompilerError::MemberRuntimeByteLimit {
                actual,
                limit: limits.max_rule_runtime_expansion_bytes,
                location,
            });
        }
        let known_owned = self.known_owned_bytes()?;
        if known_owned > limits.max_owned_template_metadata_bytes
            || known_owned > limits.max_compiler_owned_non_matcher_bytes
        {
            return Err(CompilerError::OwnedByteLimit {
                limit: limits
                    .max_owned_template_metadata_bytes
                    .min(limits.max_compiler_owned_non_matcher_bytes),
            });
        }
        Ok(())
    }

    fn known_owned_bytes(&self) -> Result<usize, CompilerError> {
        let records = self
            .templates
            .checked_mul(size_of::<CompiledTemplate>())
            .and_then(|value| value.checked_add(self.segments.checked_mul(size_of::<CompiledTemplateSegment>())?))
            .and_then(|value| value.checked_add(self.macros.checked_mul(size_of::<CompiledMacroRef>())?))
            .and_then(|value| value.checked_add(self.actions.checked_mul(size_of::<CompiledActionPlan>())?))
            .and_then(|value| value.checked_add(self.setvars.checked_mul(size_of::<CompiledSetVar>())?))
            .and_then(|value| value.checked_add(self.members.checked_mul(size_of::<CompiledMemberPlan>())?))
            .and_then(|value| value.checked_add(self.transformations.checked_mul(size_of::<TransformationKind>())?))
            .ok_or(CompilerError::PreflightArithmetic)?;
        self.metadata_bytes
            .checked_add(records)
            .ok_or(CompilerError::PreflightArithmetic)
    }

    pub(super) fn validate_peak(
        &self,
        validated_bytes: usize,
        requirement_count: usize,
        limits: CompilerLimits,
    ) -> Result<(), CompilerError> {
        let projected = self.projected_peak_bytes(validated_bytes, requirement_count)?;
        if projected > limits.max_compilation_peak_bytes {
            return Err(CompilerError::PeakByteLimit {
                limit: limits.max_compilation_peak_bytes,
            });
        }
        Ok(())
    }

    pub(super) fn projected_peak_bytes(
        &self,
        validated_bytes: usize,
        requirement_count: usize,
    ) -> Result<usize, CompilerError> {
        requirement_count
            .checked_mul(size_of::<RequirementState>())
            .and_then(|bytes| bytes.checked_add(validated_bytes))
            .and_then(|bytes| bytes.checked_add(self.known_owned_bytes().ok()?))
            .and_then(|bytes| {
                self.max_member_runtime_templates
                    .checked_mul(size_of::<(usize, usize)>())
                    .and_then(|runtime| bytes.checked_add(runtime))
            })
            .ok_or(CompilerError::PreflightArithmetic)
    }
}

pub(super) struct CompileWork {
    limits: CompilerLimits,
    validated_bytes: usize,
    pub(super) bytes: Vec<u8>,
    pub(super) asset_phrase_bytes: Vec<u8>,
    pub(super) templates: Vec<CompiledTemplate>,
    pub(super) segments: Vec<CompiledTemplateSegment>,
    pub(super) macros: Vec<CompiledMacroRef>,
    pub(super) actions: Vec<CompiledActionPlan>,
    pub(super) setvars: Vec<CompiledSetVar>,
    pub(super) members: Vec<CompiledMemberPlan>,
    pub(super) transformations: Vec<TransformationKind>,
    pub(super) assets: Vec<ResolvedAsset>,
    pub(super) phrases: Vec<CompiledPhrase>,
    pub(super) requirements: Vec<RequirementState>,
    runtime_templates: Vec<(usize, usize)>,
    requirement_cursors: [usize; 8],
    total_raw_asset_bytes: usize,
    total_phrase_bytes: usize,
    pub(super) owned_bytes: usize,
    pub(super) working_bytes: usize,
    pub(super) peak_bytes: usize,
}

impl CompileWork {
    pub(super) fn new(
        validated_bytes: usize,
        limits: CompilerLimits,
        requirements: Vec<RequirementState>,
        preflight: KnownPreflight,
    ) -> Result<Self, CompilerError> {
        let working_bytes =
            requirements
                .len()
                .checked_mul(size_of::<RequirementState>())
                .ok_or(CompilerError::PeakByteLimit {
                    limit: limits.max_compilation_peak_bytes,
                })?;
        if validated_bytes
            .checked_add(working_bytes)
            .is_none_or(|bytes| bytes > limits.max_compilation_peak_bytes)
        {
            return Err(CompilerError::PeakByteLimit {
                limit: limits.max_compilation_peak_bytes,
            });
        }
        let mut work = Self {
            limits,
            validated_bytes,
            bytes: Vec::new(),
            asset_phrase_bytes: Vec::new(),
            templates: Vec::new(),
            segments: Vec::new(),
            macros: Vec::new(),
            actions: Vec::new(),
            setvars: Vec::new(),
            members: Vec::new(),
            transformations: Vec::new(),
            assets: Vec::new(),
            phrases: Vec::new(),
            requirements,
            runtime_templates: Vec::new(),
            requirement_cursors: [0; 8],
            total_raw_asset_bytes: 0,
            total_phrase_bytes: 0,
            owned_bytes: 0,
            working_bytes,
            peak_bytes: validated_bytes + working_bytes,
        };
        reserve_exact(&mut work.bytes, preflight.metadata_bytes, "compiled bytes")?;
        reserve_exact(&mut work.templates, preflight.templates, "compiled templates")?;
        reserve_exact(&mut work.segments, preflight.segments, "compiled template segments")?;
        reserve_exact(&mut work.macros, preflight.macros, "compiled macros")?;
        reserve_exact(&mut work.actions, preflight.actions, "compiled actions")?;
        reserve_exact(&mut work.setvars, preflight.setvars, "compiled setvars")?;
        reserve_exact(&mut work.members, preflight.members, "compiled members")?;
        reserve_exact(
            &mut work.transformations,
            preflight.transformations,
            "compiled transformations",
        )?;
        reserve_exact(&mut work.assets, preflight.asset_requests, "resolved assets")?;
        reserve_exact(
            &mut work.runtime_templates,
            preflight.max_member_runtime_templates,
            "member template identities",
        )?;
        Ok(work)
    }

    pub(super) fn resolve_asset(
        &mut self,
        location: SemanticLocation,
        asset: ExternalAssetView<'_>,
        assets: &mut Option<&mut dyn PhraseAssetSource>,
    ) -> Result<(), CompilerError> {
        let request_id = self.assets.len();
        if request_id >= self.limits.max_asset_requests {
            return Err(CompilerError::AssetRequestLimit {
                actual: request_id.saturating_add(1),
                limit: self.limits.max_asset_requests,
            });
        }
        let provider = assets
            .as_deref_mut()
            .ok_or(CompilerError::MissingAssetSource { request_id, location })?;
        let mut raw = Vec::new();
        let mut sink = PhraseAssetSink {
            bytes: &mut raw,
            sticky: None,
            prior_asset_bytes: self.total_raw_asset_bytes,
            working_base_bytes: self
                .validated_bytes
                .checked_add(self.owned_bytes)
                .and_then(|bytes| bytes.checked_add(self.working_bytes))
                .ok_or(CompilerError::PeakByteLimit {
                    limit: self.limits.max_compilation_peak_bytes,
                })?,
            limits: self.limits,
            peak_bytes: &mut self.peak_bytes,
        };
        let provider_result = provider.load(
            PhraseAssetRequest {
                request_id,
                location,
                asset,
            },
            &mut sink,
        );
        if let Some(error) = sink.sticky {
            return Err(CompilerError::AssetSink {
                request_id,
                error,
                location,
            });
        }
        if let Err(failure) = provider_result {
            return Err(CompilerError::AssetSource {
                request_id,
                failure,
                location,
            });
        }
        self.total_raw_asset_bytes =
            self.total_raw_asset_bytes
                .checked_add(raw.len())
                .ok_or(CompilerError::AssetSink {
                    request_id,
                    error: PhraseAssetSinkError::Arithmetic,
                    location,
                })?;
        let phrase_start = self.phrases.len();
        self.normalize_asset(request_id, location, &raw)?;
        reserve_one(&mut self.assets, "resolved asset")?;
        self.push_working_record_charge::<ResolvedAsset>(raw.len())?;
        self.assets.push(ResolvedAsset {
            request_id,
            phrase_start,
            phrase_count: self.phrases.len() - phrase_start,
        });
        Ok(())
    }

    fn normalize_asset(
        &mut self,
        request_id: usize,
        location: SemanticLocation,
        raw: &[u8],
    ) -> Result<(), CompilerError> {
        std::str::from_utf8(raw).map_err(|_| CompilerError::AssetUtf8 { request_id, location })?;
        let initial_count = self.phrases.len();
        let mut line = 1_usize;
        let mut start = 0_usize;
        while start <= raw.len() {
            let end = raw[start..]
                .iter()
                .position(|byte| *byte == b'\n')
                .map_or(raw.len(), |offset| start + offset);
            let mut content = &raw[start..end];
            let has_crlf_terminator = end < raw.len() && content.ends_with(b"\r");
            let content_without_terminator = if has_crlf_terminator {
                &content[..content.len() - 1]
            } else {
                content
            };
            if content_without_terminator.contains(&b'\r') || (!has_crlf_terminator && content.ends_with(b"\r")) {
                return Err(CompilerError::AssetBareCarriageReturn {
                    request_id,
                    line,
                    location,
                });
            }
            if has_crlf_terminator {
                content = &content[..content.len() - 1];
            }
            content = trim_ascii_space_tab(content);
            if !content.is_empty() && content[0] != b'#' {
                let per_asset = self.phrases.len() - initial_count + 1;
                if per_asset > self.limits.max_phrases_per_asset {
                    return Err(CompilerError::AssetPhraseLimit {
                        request_id,
                        actual: per_asset,
                        limit: self.limits.max_phrases_per_asset,
                        location,
                    });
                }
                if self.phrases.len() >= self.limits.max_asset_phrases {
                    return Err(CompilerError::TotalAssetPhraseLimit {
                        actual: self.phrases.len().saturating_add(1),
                        limit: self.limits.max_asset_phrases,
                        location,
                    });
                }
                let total =
                    self.total_phrase_bytes
                        .checked_add(content.len())
                        .ok_or(CompilerError::AssetPhraseByteLimit {
                            actual: usize::MAX,
                            limit: self.limits.max_asset_phrase_bytes,
                            location,
                        })?;
                if total > self.limits.max_asset_phrase_bytes {
                    return Err(CompilerError::AssetPhraseByteLimit {
                        actual: total,
                        limit: self.limits.max_asset_phrase_bytes,
                        location,
                    });
                }
                let bytes = self.push_asset_phrase_bytes(content, raw.len())?;
                reserve_one(&mut self.phrases, "compiled phrase")?;
                self.push_working_record_charge::<CompiledPhrase>(raw.len())?;
                self.phrases.push(CompiledPhrase { bytes });
                self.total_phrase_bytes = total;
            }
            if end == raw.len() {
                break;
            }
            start = end + 1;
            line += 1;
        }
        if self.phrases.len() == initial_count {
            return Err(CompilerError::EmptyAsset { request_id, location });
        }
        Ok(())
    }

    pub(super) fn compile_member(
        &mut self,
        member: ValidatedRuleNodeView<'_, '_>,
        active: bool,
        compatibility_excluded: bool,
    ) -> Result<(), CompilerError> {
        let action_start = self.actions.len();
        debug_assert!(self.runtime_templates.is_empty());
        if compatibility_excluded {
            let state = member.resolved_state();
            let member_location = member
                .operator()
                .map(|operator| operator.location())
                .or_else(|| member.actions().next().map(|action| action.location()))
                .or_else(|| member.final_targets().next().map(|target| target.location()))
                .ok_or(CompilerError::PrivateInvariant {
                    kind: CompilerInvariant::UnexpectedActionOperand,
                })?;
            reserve_one(&mut self.members, "compiled member")?;
            self.push_record_charge::<CompiledMemberPlan>(member_location)?;
            self.members.push(CompiledMemberPlan {
                active: false,
                expensive_aggregate_actions: false,
                operator_template: None,
                target_plan: None,
                transformation_plan: None,
                phase: state.phase(),
                disruptive: state.disruptive(),
                status: state.status(),
                log: state.log(),
                audit_log: state.audit_log(),
                action_start,
                action_count: 0,
                transformation_start: self.transformations.len(),
                transformation_count: 0,
                runtime_expansion_byte_limit: self.limits.max_rule_runtime_expansion_bytes,
                skip_after_marker: None,
            });
            return Ok(());
        }
        let operator_template = if let Some(template) = member.operator().and_then(|operator| operator.template()) {
            let compiled = self.compile_template(template, member.operator().unwrap().location())?;
            self.push_runtime_template(template.identity(), compiled)?;
            Some(compiled)
        } else {
            None
        };
        let state = member.resolved_state();
        for action in member.actions() {
            if matches!(
                action.operand(),
                ActionOperandView::Ctl(CtlView::ForceRequestBodyVariable(_) | CtlView::RequestBodyProcessor(_))
            ) && state.phase() != Phase::RequestHeaders
            {
                return Err(CompilerError::CtlPhase {
                    location: action.location(),
                });
            }
            self.compile_action(action)?;
        }
        let transformation_start = self.transformations.len();
        for action in member.effective_transformations() {
            let value = action.transformation().ok_or(CompilerError::PrivateInvariant {
                kind: CompilerInvariant::UnexpectedActionOperand,
            })?;
            reserve_one(&mut self.transformations, "compiled transformation")?;
            self.push_record_charge::<TransformationKind>(action.location())?;
            self.transformations.push(value);
        }
        let runtime_bytes = self
            .runtime_templates
            .len()
            .checked_mul(self.limits.max_template_runtime_bytes)
            .ok_or(CompilerError::MemberRuntimeByteLimit {
                actual: usize::MAX,
                limit: self.limits.max_rule_runtime_expansion_bytes,
                location: member.rule().location(),
            })?;
        if runtime_bytes > self.limits.max_rule_runtime_expansion_bytes {
            return Err(CompilerError::MemberRuntimeByteLimit {
                actual: runtime_bytes,
                limit: self.limits.max_rule_runtime_expansion_bytes,
                location: member.rule().location(),
            });
        }
        let member_location = member
            .operator()
            .map(|operator| operator.location())
            .or_else(|| member.actions().next().map(|action| action.location()))
            .or_else(|| member.final_targets().next().map(|target| target.location()))
            .expect("validated member retains an execution node");
        reserve_one(&mut self.members, "compiled member")?;
        self.push_record_charge::<CompiledMemberPlan>(member_location)?;
        self.members.push(CompiledMemberPlan {
            active,
            expensive_aggregate_actions: false,
            operator_template,
            target_plan: None,
            transformation_plan: None,
            phase: state.phase(),
            disruptive: state.disruptive(),
            status: state.status(),
            log: state.log(),
            audit_log: state.audit_log(),
            action_start,
            action_count: self.actions.len() - action_start,
            transformation_start,
            transformation_count: self.transformations.len() - transformation_start,
            runtime_expansion_byte_limit: self.limits.max_rule_runtime_expansion_bytes,
            skip_after_marker: member.skip_after_marker().map(|marker| marker.ordinal()),
        });
        let runtime_working = self
            .runtime_templates
            .len()
            .checked_mul(size_of::<(usize, usize)>())
            .ok_or(CompilerError::PreflightArithmetic)?;
        self.runtime_templates.clear();
        self.working_bytes = self
            .working_bytes
            .checked_sub(runtime_working)
            .expect("member runtime logical charge was added exactly once");
        Ok(())
    }

    fn compile_action(&mut self, action: SemanticActionView<'_, '_>) -> Result<(), CompilerError> {
        if self.actions.len() >= self.limits.max_action_plans {
            return Err(CompilerError::ActionLimit {
                actual: self.actions.len().saturating_add(1),
                limit: self.limits.max_action_plans,
            });
        }
        let operand = match action.operand() {
            ActionOperandView::NoArgument => CompiledActionOperand::None,
            ActionOperandView::Local(value) => CompiledActionOperand::Local(value),
            ActionOperandView::Transformation(value) => CompiledActionOperand::Transformation(value),
            ActionOperandView::Template(template) => {
                let id = self.compile_template(template, action.location())?;
                self.push_runtime_template(template.identity(), id)?;
                CompiledActionOperand::Template(id)
            }
            ActionOperandView::Literal(value) => {
                CompiledActionOperand::Literal(self.push_bytes(value.text().as_bytes(), action.location())?)
            }
            ActionOperandView::SetVar(setvar) => {
                let key_view = setvar.key();
                let key = self.compile_template_slice(key_view, action.location())?;
                self.push_runtime_template(key_view.template_identity(), key)?;
                let value = setvar
                    .value()
                    .map(|value| {
                        let id = self.compile_template_slice(value, action.location())?;
                        Ok(id)
                    })
                    .transpose()?;
                reserve_one(&mut self.setvars, "compiled setvar")?;
                self.push_record_charge::<CompiledSetVar>(action.location())?;
                let index = self.setvars.len();
                self.setvars.push(CompiledSetVar {
                    collection: setvar.collection(),
                    operation: setvar.operation(),
                    key,
                    value,
                    constant: setvar.constant(),
                });
                if action
                    .template()
                    .is_some_and(|template| template.expansion_count() != 0)
                {
                    self.discharge(CompileRequirement::RuntimeMacro, action.location())?;
                }
                self.discharge(CompileRequirement::TransactionSetVar, action.location())?;
                CompiledActionOperand::SetVar(index)
            }
            ActionOperandView::Ctl(ctl) => {
                let compiled = match ctl {
                    CtlView::AuditEngine(_) => {
                        return Err(CompilerError::PrivateInvariant {
                            kind: CompilerInvariant::UnexpectedActionOperand,
                        });
                    }
                    CtlView::ForceRequestBodyVariable(value) => CompiledCtl::ForceRequestBodyVariable(value),
                    CtlView::RequestBodyProcessor(value) => CompiledCtl::RequestBodyProcessor(value),
                    CtlView::RuleRemoveByTag(tag) => {
                        CompiledCtl::RuleRemoveByTag(self.push_bytes(tag.text().as_bytes(), action.location())?)
                    }
                    CtlView::RuleRemoveTargetByTag { tag, target } => {
                        let tag = self.push_bytes(tag.text().as_bytes(), action.location())?;
                        let selector = match target.selector_kind() {
                            SelectorKind::None => CompiledCtlSelector::None,
                            SelectorKind::Literal | SelectorKind::Path => {
                                let text = target.selector().ok_or(CompilerError::PrivateInvariant {
                                    kind: CompilerInvariant::UnexpectedActionOperand,
                                })?;
                                let bytes = self.push_bytes(text.text().as_bytes(), action.location())?;
                                if target.selector_kind() == SelectorKind::Literal {
                                    CompiledCtlSelector::Literal(bytes)
                                } else {
                                    CompiledCtlSelector::Path(bytes)
                                }
                            }
                            SelectorKind::Regex(_) => {
                                let body = target.regex_body().ok_or(CompilerError::PrivateInvariant {
                                    kind: CompilerInvariant::UnexpectedActionOperand,
                                })?;
                                CompiledCtlSelector::RegexSource(
                                    self.push_bytes(body.text().as_bytes(), action.location())?,
                                )
                            }
                        };
                        CompiledCtl::RuleRemoveTargetByTag {
                            tag,
                            collection: target.collection(),
                            selector,
                        }
                    }
                };
                CompiledActionOperand::Ctl(compiled)
            }
            ActionOperandView::InitCol(_) => {
                return Err(CompilerError::PrivateInvariant {
                    kind: CompilerInvariant::UnexpectedActionOperand,
                });
            }
        };
        reserve_one(&mut self.actions, "compiled action")?;
        self.push_record_charge::<CompiledActionPlan>(action.location())?;
        self.actions.push(CompiledActionPlan {
            kind: action.kind(),
            operand,
            location: action.location(),
        });
        Ok(())
    }

    fn push_runtime_template(&mut self, identity: usize, compiled: usize) -> Result<(), CompilerError> {
        if self.runtime_templates.iter().any(|entry| entry.0 == identity) {
            return Err(CompilerError::PrivateInvariant {
                kind: CompilerInvariant::DuplicateRequirement,
            });
        }
        if self.runtime_templates.len() == self.runtime_templates.capacity() {
            return Err(CompilerError::PrivateInvariant {
                kind: CompilerInvariant::MissingRequirement,
            });
        }
        let new_working =
            self.working_bytes
                .checked_add(size_of::<(usize, usize)>())
                .ok_or(CompilerError::PeakByteLimit {
                    limit: self.limits.max_compilation_peak_bytes,
                })?;
        self.check_peak_parts(self.owned_bytes, new_working)?;
        self.runtime_templates.push((identity, compiled));
        self.working_bytes = new_working;
        Ok(())
    }

    fn compile_template(
        &mut self,
        template: OperandTemplateView<'_, '_>,
        location: SemanticLocation,
    ) -> Result<usize, CompilerError> {
        let expansion_count = template.expansion_count();
        let id = self.compile_segments(
            template.segments().map(|segment| {
                (
                    segment.text(),
                    match segment.kind() {
                        TemplateSegmentKind::Literal => None,
                        TemplateSegmentKind::Expansion(expansion) => Some(expansion),
                    },
                )
            }),
            location,
        )?;
        if expansion_count != 0 {
            self.discharge(CompileRequirement::RuntimeMacro, location)?;
        }
        Ok(id)
    }

    fn compile_template_slice(
        &mut self,
        template: TemplateSliceView<'_, '_>,
        location: SemanticLocation,
    ) -> Result<usize, CompilerError> {
        self.compile_segments(
            template.segments().map(|segment| {
                (
                    segment.text(),
                    match segment.kind() {
                        TemplateSegmentKind::Literal => None,
                        TemplateSegmentKind::Expansion(expansion) => Some(expansion),
                    },
                )
            }),
            location,
        )
    }

    fn compile_segments<'text, 'source>(
        &mut self,
        segments: impl Iterator<Item = (&'text str, Option<MacroExpansionView<'text, 'source>>)>,
        location: SemanticLocation,
    ) -> Result<usize, CompilerError>
    where
        'source: 'text,
    {
        if self.templates.len() >= self.limits.max_templates {
            return Err(CompilerError::TemplateLimit {
                limit: self.limits.max_templates,
                location,
            });
        }
        let segment_start = self.segments.len();
        let mut static_bytes = 0_usize;
        for (text, expansion) in segments {
            if self.segments.len() >= self.limits.max_template_segments {
                return Err(CompilerError::TemplateSegmentLimit {
                    limit: self.limits.max_template_segments,
                    location,
                });
            }
            let kind = if let Some(expansion) = expansion {
                let key = self.push_bytes(expansion.key().map_or(&[][..], |key| key.text().as_bytes()), location)?;
                reserve_one(&mut self.macros, "compiled macro")?;
                self.push_record_charge::<CompiledMacroRef>(location)?;
                let index = self.macros.len();
                self.macros.push(CompiledMacroRef {
                    source: expansion.source(),
                    key,
                });
                CompiledSegmentKind::Macro(index)
            } else {
                static_bytes = static_bytes
                    .checked_add(text.len())
                    .ok_or(CompilerError::TemplateStaticByteLimit {
                        actual: usize::MAX,
                        limit: self.limits.max_template_static_bytes,
                        location,
                    })?;
                CompiledSegmentKind::Literal(self.push_bytes(text.as_bytes(), location)?)
            };
            reserve_one(&mut self.segments, "compiled template segment")?;
            self.push_record_charge::<CompiledTemplateSegment>(location)?;
            self.segments.push(CompiledTemplateSegment { kind });
        }
        if static_bytes > self.limits.max_template_static_bytes {
            return Err(CompilerError::TemplateStaticByteLimit {
                actual: static_bytes,
                limit: self.limits.max_template_static_bytes,
                location,
            });
        }
        if static_bytes > self.limits.max_template_runtime_bytes {
            return Err(CompilerError::TemplateRuntimeByteLimit {
                limit: self.limits.max_template_runtime_bytes,
                location,
            });
        }
        reserve_one(&mut self.templates, "compiled template")?;
        self.push_record_charge::<CompiledTemplate>(location)?;
        let id = self.templates.len();
        self.templates.push(CompiledTemplate {
            segment_start,
            segment_count: self.segments.len() - segment_start,
            static_bytes,
            runtime_byte_limit: self.limits.max_template_runtime_bytes,
        });
        Ok(id)
    }

    pub(super) fn discharge(
        &mut self,
        kind: CompileRequirement,
        location: SemanticLocation,
    ) -> Result<(), CompilerError> {
        let category = requirement_category(kind);
        let key = requirement_key(kind, location);
        let Ok(index) = self.requirements.binary_search_by_key(&key, |requirement| {
            requirement_key(requirement.kind, requirement.location)
        }) else {
            return Err(CompilerError::PrivateInvariant {
                kind: CompilerInvariant::MissingRequirement,
            });
        };
        let requirement = &mut self.requirements[index];
        if requirement.discharged {
            return Err(CompilerError::PrivateInvariant {
                kind: CompilerInvariant::DuplicateRequirement,
            });
        }
        requirement.discharged = true;
        self.requirement_cursors[category] = self.requirement_cursors[category].saturating_add(1);
        Ok(())
    }

    pub(super) fn verify_a_requirements(&self) -> Result<(), CompilerError> {
        for requirement in &self.requirements {
            if matches!(
                requirement.kind,
                CompileRequirement::RuntimeMacro
                    | CompileRequirement::ExternalPhraseAsset
                    | CompileRequirement::TransactionSetVar
            ) && !requirement.discharged
            {
                return Err(CompilerError::PrivateInvariant {
                    kind: CompilerInvariant::MissingRequirement,
                });
            }
        }
        Ok(())
    }

    fn push_bytes(&mut self, bytes: &[u8], _location: SemanticLocation) -> Result<ByteRange, CompilerError> {
        let new_owned = self
            .owned_bytes
            .checked_add(bytes.len())
            .ok_or(CompilerError::OwnedByteLimit {
                limit: self.limits.max_compiler_owned_non_matcher_bytes,
            })?;
        if new_owned > self.limits.max_compiler_owned_non_matcher_bytes
            || new_owned > self.limits.max_owned_template_metadata_bytes
        {
            return Err(CompilerError::OwnedByteLimit {
                limit: self
                    .limits
                    .max_compiler_owned_non_matcher_bytes
                    .min(self.limits.max_owned_template_metadata_bytes),
            });
        }
        self.check_peak(new_owned)?;
        self.bytes
            .try_reserve_exact(bytes.len())
            .map_err(|_| CompilerError::Allocation {
                operation: "compiled bytes",
            })?;
        let range = ByteRange {
            start: self.bytes.len(),
            len: bytes.len(),
        };
        self.bytes.extend_from_slice(bytes);
        self.owned_bytes = new_owned;
        Ok(range)
    }

    fn push_record_charge<T>(&mut self, _location: SemanticLocation) -> Result<(), CompilerError> {
        let new_owned = self
            .owned_bytes
            .checked_add(size_of::<T>())
            .ok_or(CompilerError::OwnedByteLimit {
                limit: self.limits.max_compiler_owned_non_matcher_bytes,
            })?;
        if new_owned > self.limits.max_compiler_owned_non_matcher_bytes {
            return Err(CompilerError::OwnedByteLimit {
                limit: self.limits.max_compiler_owned_non_matcher_bytes,
            });
        }
        self.check_peak(new_owned)?;
        self.owned_bytes = new_owned;
        Ok(())
    }

    fn push_asset_phrase_bytes(&mut self, bytes: &[u8], raw_live_bytes: usize) -> Result<ByteRange, CompilerError> {
        let new_working = self
            .working_bytes
            .checked_add(bytes.len())
            .ok_or(CompilerError::PeakByteLimit {
                limit: self.limits.max_compilation_peak_bytes,
            })?;
        self.check_peak_parts_with_raw(self.owned_bytes, new_working, raw_live_bytes)?;
        self.asset_phrase_bytes
            .try_reserve_exact(bytes.len())
            .map_err(|_| CompilerError::Allocation {
                operation: "asset phrase bytes",
            })?;
        let range = ByteRange {
            start: self.asset_phrase_bytes.len(),
            len: bytes.len(),
        };
        self.asset_phrase_bytes.extend_from_slice(bytes);
        self.working_bytes = new_working;
        Ok(range)
    }

    fn push_working_record_charge<T>(&mut self, raw_live_bytes: usize) -> Result<(), CompilerError> {
        let new_working = self
            .working_bytes
            .checked_add(size_of::<T>())
            .ok_or(CompilerError::PeakByteLimit {
                limit: self.limits.max_compilation_peak_bytes,
            })?;
        self.check_peak_parts_with_raw(self.owned_bytes, new_working, raw_live_bytes)?;
        self.working_bytes = new_working;
        Ok(())
    }

    fn check_peak(&mut self, owned: usize) -> Result<(), CompilerError> {
        self.check_peak_parts(owned, self.working_bytes)
    }

    fn check_peak_parts(&mut self, owned: usize, working: usize) -> Result<(), CompilerError> {
        self.check_peak_parts_with_raw(owned, working, 0)
    }

    fn check_peak_parts_with_raw(
        &mut self,
        owned: usize,
        working: usize,
        raw_live_bytes: usize,
    ) -> Result<(), CompilerError> {
        let peak = self
            .validated_bytes
            .checked_add(owned)
            .and_then(|bytes| bytes.checked_add(working))
            .and_then(|bytes| bytes.checked_add(raw_live_bytes))
            .ok_or(CompilerError::PeakByteLimit {
                limit: self.limits.max_compilation_peak_bytes,
            })?;
        self.peak_bytes = self.peak_bytes.max(peak);
        if peak > self.limits.max_compilation_peak_bytes {
            return Err(CompilerError::PeakByteLimit {
                limit: self.limits.max_compilation_peak_bytes,
            });
        }
        Ok(())
    }
}
