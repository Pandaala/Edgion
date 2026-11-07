//! Phase entry points: request head/body (snapshot, prefix and streaming), response head/window and logging.

use super::*;

impl<'generation> Transaction<'generation> {
    pub fn generation(&self) -> &CompiledGeneration {
        &self.generation
    }

    pub fn retained_bytes(&self) -> usize {
        self.request
            .retained_bytes
            .saturating_add(self.scratch_bytes)
            .saturating_add(self.tx.retained_bytes)
            .saturating_add(self.response.as_ref().map_or(0, ResponseState::total_retained_bytes))
            .saturating_add(
                self.request_body_stream
                    .as_ref()
                    .map_or(0, |state| state.retained_bytes),
            )
    }

    pub fn next_phase(&self) -> Option<Phase> {
        phase_from_number(self.next_phase)
    }

    pub fn tx_value(&self, key: &[u8]) -> Option<&[u8]> {
        self.tx
            .observed_get(key, &self.request)
            .ok()
            .flatten()?
            .observed_bytes(&self.request)
            .ok()
    }

    pub const fn request_body_error(&self) -> Option<BodyError> {
        self.request_body_error
    }

    /// Narrow source admission after header controls and before body publication.
    /// Independent parsing, retained-state and execution budgets are unchanged.
    pub fn constrain_request_body_bytes(&mut self, limit: usize) -> Result<(), RuntimeError> {
        self.preflight_request_body(crate::seclang::BodyProcessorSelection::Auto)?;
        if self.request_body_stream.is_some() || self.request_prefix_completion.is_some() || self.json_budget.is_some()
        {
            return Err(RuntimeError::RequestBodyAlreadyPublished);
        }
        if limit > self.limits.body.max_body_bytes {
            return Err(RuntimeError::GenerationInvariant);
        }
        self.limits.body.max_body_bytes = limit;
        Ok(())
    }

    pub fn execute_request_headers(&mut self) -> Result<PhaseOutcome, RuntimeError> {
        self.execute_phase(Phase::RequestHeaders)
    }

    pub(super) fn publish_non_json_body(&mut self, body: RequestBody) -> Result<(), RuntimeError> {
        self.ensure_json_healthy()?;
        if self.next_phase() != Some(Phase::RequestBody) {
            return Err(RuntimeError::PhaseOrder {
                expected: self.next_phase(),
                actual: Phase::RequestBody,
            });
        }
        if self.request.body.is_some() {
            return Err(RuntimeError::RequestBodyAlreadyPublished);
        }
        let content_type = self
            .request
            .first_named(CollectionKind::RequestHeaders, b"content-type", true);
        let head_args_combined_size = self
            .request
            .first_value(CollectionKind::ArgsCombinedSize)
            .ok_or(RuntimeError::GenerationInvariant)
            .and_then(parse_usize)?;

        let raw_policy = self.tx.force_request_body_variable.map(|value| value == OnOff::On);
        let publication = if raw_policy.is_some() {
            build_publication_with_raw_policy(
                &body,
                content_type,
                head_args_combined_size,
                self.limits.body,
                raw_policy,
            )
        } else {
            build_publication(&body, content_type, head_args_combined_size, self.limits.body)
        };
        let (publication, body_error) = match publication {
            Ok(publication) => (publication, None),
            Err(error @ (BodyError::Malformed { .. } | BodyError::MultipartBoundary))
                if self.rule_engine_mode == RuleEngineMode::DetectionOnly =>
            {
                (
                    if raw_policy.is_some() {
                        build_fallback_publication_with_raw_policy(
                            &body,
                            content_type,
                            head_args_combined_size,
                            self.limits.body,
                            raw_policy,
                        )
                    } else {
                        build_fallback_publication(&body, content_type, head_args_combined_size, self.limits.body)
                    }
                    .map_err(|kind| RuntimeError::Body { kind })?,
                    Some(error),
                )
            }
            Err(kind) => return Err(RuntimeError::Body { kind }),
        };
        self.publish_body_state(RequestBodyPublication::Materialized(publication), body_error)
    }

    pub(super) fn publish_body_state(
        &mut self,
        publication: RequestBodyPublication<'generation>,
        body_error: Option<BodyError>,
    ) -> Result<(), RuntimeError> {
        let entry_count = self
            .request
            .entry_count
            .checked_add(publication.entry_count())
            .ok_or(RuntimeError::Arithmetic)?;
        if entry_count > self.limits.max_collection_entries {
            return Err(RuntimeError::CollectionEntryLimit {
                limit: self.limits.max_collection_entries,
            });
        }
        let request_retained = self
            .request
            .retained_bytes
            .checked_add(publication.retained_bytes())
            .ok_or(RuntimeError::Arithmetic)?;
        let total_retained = request_retained
            .checked_add(self.scratch_bytes)
            .and_then(|value| value.checked_add(self.tx.retained_bytes))
            .ok_or(RuntimeError::Arithmetic)?;
        if total_retained > self.limits.max_retained_bytes {
            return Err(RuntimeError::RetainedByteLimit {
                limit: self.limits.max_retained_bytes,
            });
        }
        self.request.body = Some(publication);
        self.request_body_error = body_error;
        self.request.entry_count = entry_count;
        self.request.retained_bytes = request_retained;
        Ok(())
    }

    pub(super) fn ensure_json_healthy(&self) -> Result<(), RuntimeError> {
        if let Some(error) = self.json_failure {
            return Err(error);
        }
        if let Some(error) = self.json_budget.as_ref().and_then(InspectionBudget::failure) {
            return Err(inspection_error(&self.request, map_store_error(error)));
        }
        Ok(())
    }
    pub fn execute_request_body(&mut self) -> Result<PhaseOutcome, RuntimeError> {
        self.ensure_json_healthy()?;
        if self.request.body.is_none() {
            return Err(RuntimeError::RequestBodyNotPublished);
        }
        self.execute_phase(Phase::RequestBody)
    }

    /// Decide phase two over borrowed prefix bytes without publishing a complete body.
    pub fn execute_request_body_prefix(
        &mut self,
        prefix: &[u8],
        limit: usize,
        completion: RequestBodyPrefixCompletion,
    ) -> Result<PhaseOutcome, RuntimeError> {
        self.ensure_json_healthy()?;
        if self.next_phase() != Some(Phase::RequestBody) {
            return Err(RuntimeError::PhaseOrder {
                expected: self.next_phase(),
                actual: Phase::RequestBody,
            });
        }
        self.generation
            .validate_request_body_prefix()
            .map_err(|_| RuntimeError::GenerationInvariant)?;
        if self.request.body.is_some()
            || self.request_body_stream.is_some()
            || limit == 0
            || limit > MAX_REQUEST_BODY_PREFIX_BYTES
            || prefix.len() > limit
            || (completion == RequestBodyPrefixCompletion::WindowFilled && prefix.len() != limit)
            || (completion == RequestBodyPrefixCompletion::NotRequired
                && (!prefix.is_empty() || self.generation.requires_complete_request_body()))
        {
            return Err(RuntimeError::GenerationInvariant);
        }
        if prefix.len() > self.limits.body.max_body_bytes {
            return Err(RuntimeError::Body {
                kind: BodyError::ByteLimit {
                    actual: prefix.len(),
                    limit: self.limits.body.max_body_bytes,
                },
            });
        }
        if completion == RequestBodyPrefixCompletion::NotRequired {
            let outcome = self.execute_phase(Phase::RequestBody)?;
            self.request_prefix_completion = Some(completion);
            return Ok(outcome);
        }
        let outcome = self.execute_phase_with_prefix(Phase::RequestBody, true, Some(prefix));
        if outcome.is_ok() {
            self.request_prefix_completion = Some(completion);
        }
        outcome
    }

    pub(super) fn evaluate_prefix_member(
        &mut self,
        id: usize,
        prefix: &[u8],
        work: &mut usize,
    ) -> Result<bool, RuntimeError> {
        self.ordinary_scan_cache.clear();
        self.borrowed_transform_memo.clear();
        let member = self.generation.members[id];
        let transformations = member
            .transformation_plan
            .map(|id| {
                let plan = self.generation.transformation_plans[id];
                &self.generation.transformations
                    [plan.transformation_start..plan.transformation_start + plan.transformation_count]
            })
            .unwrap_or_default();
        let transformed = apply_transformations_borrowed(
            prefix,
            transformations,
            TransformLimits {
                max_output_bytes: self.limits.max_transformation_output_bytes,
                max_work_bytes: self.limits.max_transformation_work_bytes,
            },
        )
        .map_err(map_transform_error)?;
        *work = work
            .checked_add(prefix.len())
            .and_then(|work| work.checked_add(transformed.len()))
            .ok_or(RuntimeError::Arithmetic)?;
        if *work > self.limits.max_operator_work_bytes {
            return Err(RuntimeError::OperatorWorkLimit {
                limit: self.limits.max_operator_work_bytes,
            });
        }
        let plan = self.generation.operator_plans[id].ok_or(RuntimeError::GenerationInvariant)?;
        let mut context = SingleInputContext {
            generation: &self.generation,
            request: &self.request,
            tx: &self.tx,
            group_scratch: &mut self.group_scratch,
            boolean_scratch: &mut self.boolean_scratch,
            capture_scratch: &mut self.capture_scratch,
            scan_cache: None,
            json_budget: self.json_budget.as_mut(),
            transform_memo: None,
        };
        evaluate_operator(
            &mut context,
            plan,
            &transformed,
            member.runtime_expansion_byte_limit,
            self.evaluation_mode,
        )
        .map(|(matched, _)| matched)
    }

    pub const fn request_prefix_completion(&self) -> Option<RequestBodyPrefixCompletion> {
        self.request_prefix_completion
    }

    /// Prepare the exact constant-memory raw-body lane selected by the compiler.
    pub fn start_request_body_stream(&mut self) -> Result<(), RuntimeError> {
        self.ensure_json_healthy()?;
        if self.next_phase() != Some(Phase::RequestBody) {
            return Err(RuntimeError::PhaseOrder {
                expected: self.next_phase(),
                actual: Phase::RequestBody,
            });
        }
        if self.request.body.is_some() || self.request_body_stream.is_some() {
            return Err(RuntimeError::RequestBodyAlreadyPublished);
        }
        if !self.tx.removed_units.is_empty()
            || !self.tx.target_exclusions.is_empty()
            || self.tx.request_body_processor.is_some()
            || self.tx.force_request_body_variable.is_some()
        {
            return Err(RuntimeError::RequestBodyStreamCtlUnavailable);
        }
        let summary = self.generation.request_body_execution_summary();
        if summary.streaming_structured_rule_units() != 0
            || summary.bounded_snapshot_rule_units() != 0
            || summary.unsupported_rule_units() != 0
        {
            return Err(RuntimeError::GenerationInvariant);
        }
        let mut groups = Vec::new();
        groups
            .try_reserve_exact(self.generation.groups.len())
            .map_err(|_| RuntimeError::Allocation)?;
        let mut retained_bytes = 0_usize;
        for group in &self.generation.groups {
            let mapping_end = group
                .mapping_start
                .checked_add(group.mapping_count)
                .ok_or(RuntimeError::Arithmetic)?;
            let mappings = self
                .generation
                .pattern_mappings
                .get(group.mapping_start..mapping_end)
                .ok_or(RuntimeError::GenerationInvariant)?;
            let streaming = group.key.phase == Phase::RequestBody
                && !mappings.is_empty()
                && mappings
                    .iter()
                    .all(|mapping| is_streaming_raw_member(&self.generation, mapping.member));
            if streaming {
                let state = group.matcher.create_stream_state().map_err(map_matcher_error)?;
                retained_bytes = retained_bytes
                    .checked_add(state.memory_usage())
                    .ok_or(RuntimeError::Arithmetic)?;
                groups.push(Some(state));
            } else {
                groups.push(None);
            }
        }
        let member_positive = vec![false; self.generation.members.len()];
        let mut phase_two_units = self.generation.schedules[phase_index(Phase::RequestBody)]
            .iter()
            .filter_map(|entry| match entry {
                CompiledScheduleEntry::RuleUnit(unit_index) => Some(*unit_index),
                CompiledScheduleEntry::MarkerSentinel(_) => None,
            });
        let early_positive_member = phase_two_units
            .next()
            .and_then(|unit_index| phase_two_units.next().is_none().then_some(unit_index));
        let early_positive_member = early_positive_member.and_then(|unit_index| {
            let unit = self.generation.units.get(unit_index)?;
            if unit.member_count != 1 || !is_streaming_raw_member(&self.generation, unit.member_start) {
                return None;
            }
            self.generation
                .operator_plans
                .get(unit.member_start)
                .and_then(|plan| *plan)
                .is_some_and(|plan| !plan.negated)
                .then_some(unit.member_start)
        });
        retained_bytes = retained_bytes
            .checked_add(member_positive.capacity().saturating_mul(size_of::<bool>()))
            .and_then(|value| {
                value.checked_add(
                    groups
                        .capacity()
                        .saturating_mul(size_of::<Option<MatcherStreamState>>()),
                )
            })
            .ok_or(RuntimeError::Arithmetic)?;
        if self.retained_bytes().saturating_add(retained_bytes) > self.limits.max_retained_bytes {
            return Err(RuntimeError::RetainedByteLimit {
                limit: self.limits.max_retained_bytes,
            });
        }
        self.request_body_stream = Some(RequestBodyStreamState {
            groups,
            member_positive,
            consumed_bytes: 0,
            operator_work_bytes: 0,
            early_positive_member,
            retained_bytes,
        });
        Ok(())
    }

    /// Scan one raw request-body chunk. `Some` is returned exactly once at EOS.
    pub fn execute_request_body_stream_chunk(
        &mut self,
        chunk: &[u8],
        end_of_stream: bool,
    ) -> Result<Option<PhaseOutcome>, RuntimeError> {
        self.ensure_json_healthy()?;
        let mut early_finalize = false;
        {
            let stream = self
                .request_body_stream
                .as_mut()
                .ok_or(RuntimeError::RequestBodyNotPublished)?;
            let consumed_bytes = stream
                .consumed_bytes
                .checked_add(chunk.len())
                .ok_or(RuntimeError::Arithmetic)?;
            if consumed_bytes > self.limits.body.max_body_bytes {
                return Err(RuntimeError::Body {
                    kind: BodyError::ByteLimit {
                        actual: consumed_bytes,
                        limit: self.limits.body.max_body_bytes,
                    },
                });
            }
            stream.consumed_bytes = consumed_bytes;
            for (group_index, state) in stream.groups.iter_mut().enumerate() {
                let Some(state) = state else { continue };
                stream.operator_work_bytes = stream
                    .operator_work_bytes
                    .checked_add(chunk.len())
                    .ok_or(RuntimeError::Arithmetic)?;
                if stream.operator_work_bytes > self.limits.max_operator_work_bytes {
                    return Err(RuntimeError::OperatorWorkLimit {
                        limit: self.limits.max_operator_work_bytes,
                    });
                }
                let group = self
                    .generation
                    .groups
                    .get(group_index)
                    .ok_or(RuntimeError::GenerationInvariant)?;
                let results = group
                    .matcher
                    .scan_stream_chunk(state, chunk, end_of_stream)
                    .map_err(map_matcher_error)?;
                for pattern_id in results.iter() {
                    let mapping_index = group
                        .mapping_start
                        .checked_add(pattern_id.as_usize())
                        .ok_or(RuntimeError::Arithmetic)?;
                    let mapping = self
                        .generation
                        .pattern_mappings
                        .get(mapping_index)
                        .ok_or(RuntimeError::GenerationInvariant)?;
                    let matched = stream
                        .member_positive
                        .get_mut(mapping.member)
                        .ok_or(RuntimeError::GenerationInvariant)?;
                    *matched = true;
                    early_finalize |= stream.early_positive_member == Some(mapping.member);
                }
            }
        }
        if !end_of_stream && !early_finalize {
            return Ok(None);
        }
        let stream = self
            .request_body_stream
            .take()
            .ok_or(RuntimeError::RequestBodyNotPublished)?;
        let mut overrides = vec![None; self.generation.members.len()];
        for (member_index, positive) in stream.member_positive.into_iter().enumerate() {
            if !is_streaming_raw_member(&self.generation, member_index) {
                continue;
            }
            let operator = self
                .generation
                .operator_plans
                .get(member_index)
                .and_then(|plan| *plan)
                .ok_or(RuntimeError::GenerationInvariant)?;
            overrides[member_index] = Some(positive != operator.negated);
        }
        self.stream_member_matches = Some(overrides);
        self.publish_request_body(RequestBody::new(bytes::Bytes::new(), BodyProcessorSelection::Raw))?;
        let outcome = self.execute_request_body();
        self.stream_member_matches = None;
        outcome.map(Some)
    }

    pub fn publish_response_head(&mut self, response: ResponseHead<'_>) -> Result<(), RuntimeError> {
        if self.next_phase() != Some(Phase::ResponseHeaders) {
            return Err(RuntimeError::PhaseOrder {
                expected: self.next_phase(),
                actual: Phase::ResponseHeaders,
            });
        }
        if self.response.is_some() {
            return Err(RuntimeError::ResponseAlreadyPublished);
        }
        let response = ResponseState::build(response, self.limits)?;
        let total_retained = self
            .retained_bytes()
            .checked_add(response.total_retained_bytes())
            .ok_or(RuntimeError::Arithmetic)?;
        if total_retained > self.limits.max_retained_bytes {
            return Err(RuntimeError::RetainedByteLimit {
                limit: self.limits.max_retained_bytes,
            });
        }
        self.response = Some(response);
        Ok(())
    }

    pub fn execute_response_headers(&mut self) -> Result<PhaseOutcome, RuntimeError> {
        if self.response.is_none() {
            return Err(RuntimeError::ResponseNotPublished);
        }
        self.execute_phase(Phase::ResponseHeaders)
    }

    pub fn execute_response_window(&mut self, window: ResponseWindow<'_>) -> Result<PhaseOutcome, RuntimeError> {
        if self.next_phase() != Some(Phase::ResponseBody) {
            return Err(RuntimeError::PhaseOrder {
                expected: self.next_phase(),
                actual: Phase::ResponseBody,
            });
        }
        if window.bytes.len() > self.limits.max_response_window_bytes {
            return Err(RuntimeError::ResponseWindowByteLimit {
                actual: window.bytes.len(),
                limit: self.limits.max_response_window_bytes,
            });
        }
        if self.response.is_none() {
            return Err(RuntimeError::ResponseNotPublished);
        }
        let bytes = owned_copy(window.bytes)?;
        let total_retained = self
            .retained_bytes()
            .checked_add(bytes.len())
            .ok_or(RuntimeError::Arithmetic)?;
        if total_retained > self.limits.max_retained_bytes {
            return Err(RuntimeError::RetainedByteLimit {
                limit: self.limits.max_retained_bytes,
            });
        }
        self.response.as_mut().ok_or(RuntimeError::ResponseNotPublished)?.window = Some(bytes);
        let result = self.execute_phase_with_advance(Phase::ResponseBody, window.end_of_stream);
        if let Some(response) = &mut self.response {
            response.window = None;
        }
        result
    }

    pub fn execute_logging(&mut self) -> Result<PhaseOutcome, RuntimeError> {
        self.execute_phase(Phase::Logging)
    }

    fn execute_phase(&mut self, phase: Phase) -> Result<PhaseOutcome, RuntimeError> {
        self.execute_phase_with_advance(phase, true)
    }

    fn execute_phase_with_advance(&mut self, phase: Phase, advance: bool) -> Result<PhaseOutcome, RuntimeError> {
        self.execute_phase_with_prefix(phase, advance, None)
    }

    fn execute_phase_with_prefix(
        &mut self,
        phase: Phase,
        advance: bool,
        prefix: Option<&[u8]>,
    ) -> Result<PhaseOutcome, RuntimeError> {
        self.ordinary_scan_cache.clear();
        self.borrowed_transform_memo.clear();
        self.ensure_json_healthy()?;
        let mut prefix_work = 0_usize;
        let expected = self.next_phase();
        if expected != Some(phase) {
            return Err(RuntimeError::PhaseOrder {
                expected,
                actual: phase,
            });
        }
        let mut outcome = PhaseOutcome {
            findings: Vec::new(),
            omitted_findings: 0,
            omitted_loggable_findings: 0,
            intervention: None,
            evaluated_units: 0,
        };
        let schedule_index = phase_index(phase);
        let schedule_len = self.generation.schedules[schedule_index].len();
        let policy_prunes = self.generation.has_expensive_checks && !self.tx.check_policy.expensive_checks;
        let mut schedule_ordinal = 0_usize;
        while schedule_ordinal < schedule_len {
            let entry = self.generation.schedules[schedule_index][schedule_ordinal];
            schedule_ordinal = schedule_ordinal.checked_add(1).ok_or(RuntimeError::Arithmetic)?;
            let CompiledScheduleEntry::RuleUnit(unit_id) = entry else {
                continue;
            };
            if self.tx.removed_units.binary_search(&unit_id).is_ok()
                || (policy_prunes && self.generation.units[unit_id].expensive_check)
            {
                continue;
            }
            outcome.evaluated_units = outcome.evaluated_units.checked_add(1).ok_or(RuntimeError::Arithmetic)?;
            if outcome.evaluated_units > self.limits.max_operator_evaluations {
                return Err(RuntimeError::OperatorEvaluationLimit {
                    limit: self.limits.max_operator_evaluations,
                });
            }
            let prefix_match = if let Some(prefix) = prefix {
                let unit = self
                    .generation
                    .units
                    .get(unit_id)
                    .ok_or(RuntimeError::GenerationInvariant)?;
                if unit.member_count != 1 {
                    return Err(RuntimeError::GenerationInvariant);
                }
                if self
                    .generation
                    .member_requires_request_body(self.generation.members[unit.member_start])
                {
                    Some(self.evaluate_prefix_member(unit.member_start, prefix, &mut prefix_work)?)
                } else {
                    None
                }
            } else {
                None
            };
            if self.evaluate_unit(unit_id, prefix_match)? {
                let unit = *self
                    .generation
                    .units
                    .get(unit_id)
                    .ok_or(RuntimeError::GenerationInvariant)?;
                if outcome.findings.len() < self.limits.max_findings {
                    if outcome.findings.len() == outcome.findings.capacity() {
                        let remaining = self.limits.max_findings.saturating_sub(outcome.findings.len());
                        let additional = remaining.min(outcome.findings.capacity().max(4));
                        outcome
                            .findings
                            .try_reserve_exact(additional)
                            .map_err(|_| RuntimeError::Allocation)?;
                    }
                    let (severity, tag_count) = self.finding_metadata(unit)?;
                    outcome.findings.push(RuleFinding {
                        rule_id: unit.id,
                        log: unit.log,
                        audit_log: unit.audit_log,
                        severity,
                        tag_count,
                    });
                } else {
                    outcome.omitted_findings = outcome
                        .omitted_findings
                        .checked_add(1)
                        .ok_or(RuntimeError::Arithmetic)?;
                    if unit.log || unit.audit_log {
                        outcome.omitted_loggable_findings = outcome
                            .omitted_loggable_findings
                            .checked_add(1)
                            .ok_or(RuntimeError::Arithmetic)?;
                    }
                }
                if unit.disruptive == ResolvedDisruptive::Deny && self.rule_engine_mode == RuleEngineMode::On {
                    outcome.intervention = Some(Intervention {
                        rule_id: unit.id,
                        status: unit.status.unwrap_or(403),
                    });
                    break;
                }
                schedule_ordinal = self.next_schedule_ordinal(unit, schedule_index, schedule_ordinal)?;
            }
        }
        if advance {
            self.next_phase = self.next_phase.checked_add(1).ok_or(RuntimeError::Arithmetic)?;
        }
        Ok(outcome)
    }
}
