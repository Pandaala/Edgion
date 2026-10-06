//! Canonical JSON publication and ordered consumption of provisional results.
use super::json_inspection::{ResultLimits, SealedResults, StoreError};
use super::json_inspection_collector::{CollectorError, JsonCollector};
use super::*;
use crate::seclang::body::BodyAccounting;
#[cfg(test)]
use crate::seclang::measurements::{self, Event};
use crate::seclang::{
    BodyProcessorSelection, JsonChecker, JsonCheckerError, JsonPublicationSummary, JsonSemanticSummary,
    JsonStreamError, JsonStreamInput, JsonStreamLimits, JsonStructureLimits,
};
use bytes::Bytes;

#[derive(Debug, Clone, Copy)]
pub struct JsonInspectionLimits {
    pub lexical: JsonStreamLimits,
    pub structure: JsonStructureLimits,
    pub results: ResultLimits,
}
impl Default for JsonInspectionLimits {
    fn default() -> Self {
        Self {
            lexical: JsonStreamLimits {
                max_source_bytes: 16 << 20,
                max_raw_token_bytes: 16 << 20,
                max_decoded_bytes: 32 << 20,
                max_retained_bytes: 32 << 20,
                max_events: 131_072,
                max_depth: 64,
                max_work: 128 << 20,
            },
            structure: JsonStructureLimits::default(),
            results: ResultLimits::default(),
        }
    }
}
pub(super) struct JsonPublication<'g> {
    pub results: Option<SealedResults<'g>>,
    pub source: Option<Bytes>,
    length: ([u8; 40], u8),
    combined: ([u8; 40], u8),
    pub summary: JsonPublicationSummary,
}
impl JsonPublication<'_> {
    pub(super) fn metadata_value(&self, collection: CollectionKind) -> Option<&[u8]> {
        match collection {
            CollectionKind::RequestBody => self.source.as_deref(),
            CollectionKind::RequestBodyLength => Some(&self.length.0[usize::from(self.length.1)..]),
            CollectionKind::ReqbodyProcessor => Some(b"JSON"),
            CollectionKind::ArgsCombinedSize => Some(&self.combined.0[usize::from(self.combined.1)..]),
            _ => None,
        }
    }
    pub(super) fn results(&self) -> Result<&SealedResults<'_>, RuntimeError> {
        self.results.as_ref().ok_or(RuntimeError::JsonInspectionState)
    }
    pub(super) fn argument_count(&self) -> usize {
        self.results
            .as_ref()
            .map_or(0, |results| results.summary.argument_count)
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsonResultLimit {
    RetainedBytes,
    Records,
    Candidates,
    Work,
    Deferred,
}
pub(super) fn map_store_error(error: StoreError) -> RuntimeError {
    match error {
        StoreError::Allocation => RuntimeError::Allocation,
        StoreError::Arithmetic => RuntimeError::Arithmetic,
        StoreError::Capacity => RuntimeError::JsonResultLimit {
            kind: JsonResultLimit::RetainedBytes,
        },
        StoreError::Records => RuntimeError::JsonResultLimit {
            kind: JsonResultLimit::Records,
        },
        StoreError::Candidates => RuntimeError::JsonResultLimit {
            kind: JsonResultLimit::Candidates,
        },
        StoreError::Work => RuntimeError::JsonResultLimit {
            kind: JsonResultLimit::Work,
        },
        StoreError::Deferred => RuntimeError::JsonResultLimit {
            kind: JsonResultLimit::Deferred,
        },
        StoreError::Identity | StoreError::State | StoreError::Generation => RuntimeError::JsonInspectionState,
    }
}
fn map_collector_error(error: CollectorError) -> RuntimeError {
    match error {
        CollectorError::Store(error) => map_store_error(error),
        CollectorError::Runtime(error) => error,
        CollectorError::State => RuntimeError::JsonInspectionState,
    }
}
fn malformed(error: JsonCheckerError) -> bool {
    matches!(
        error,
        JsonCheckerError::Lexical(
            JsonStreamError::Syntax
                | JsonStreamError::Utf8
                | JsonStreamError::Escape
                | JsonStreamError::Number
                | JsonStreamError::Incomplete
        )
    )
}
impl<'generation> Transaction<'generation> {
    pub fn preflight_request_body(
        &self,
        requested: BodyProcessorSelection,
    ) -> Result<BodyProcessorSelection, RuntimeError> {
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
        let requested = self.tx.request_body_processor.unwrap_or(requested);
        Ok(if requested == BodyProcessorSelection::Auto {
            processor_from_content_type(
                self.request
                    .first_named(CollectionKind::RequestHeaders, b"content-type", true),
            )
        } else {
            requested
        })
    }
    /// Publish one owned complete body using the default JSON inspection profile.
    pub fn publish_request_body(&mut self, body: RequestBody) -> Result<(), RuntimeError> {
        self.publish_request_body_with_json_limits(body, JsonInspectionLimits::default())
    }

    /// Publish one owned complete body with explicit JSON physical limits.
    /// The effective processor is resolved after request-header actions.
    pub fn publish_request_body_with_json_limits(
        &mut self,
        body: RequestBody,
        limits: JsonInspectionLimits,
    ) -> Result<(), RuntimeError> {
        let (source, requested) = body.into_parts();
        let processor = self.preflight_request_body(requested)?;
        if processor != BodyProcessorSelection::Json {
            return self.publish_non_json_body(RequestBody::new(source, processor));
        }
        let result = self.inspect_owned_json(source, limits);
        if let Err(error) = result {
            self.json_failure.get_or_insert(error);
        }
        result
    }
    fn inspect_owned_json(&mut self, source: Bytes, limits: JsonInspectionLimits) -> Result<(), RuntimeError> {
        if let Some(unsupported) = self.generation.json_inspection.unsupported.first() {
            return Err(RuntimeError::JsonUnsupported {
                rule_id: unsupported.rule_id,
            });
        }
        let head_combined = parse_usize(
            self.request
                .first_value(CollectionKind::ArgsCombinedSize)
                .ok_or(RuntimeError::GenerationInvariant)?,
        )?;
        let raw = self
            .tx
            .force_request_body_variable
            .map(|value| value == super::super::OnOff::On);
        let mut accounting = BodyAccounting::json_prelude(source.len(), self.limits.body, raw.unwrap_or(false))
            .map_err(|kind| RuntimeError::Body { kind })?;
        let mut results = None;
        let mut budget;
        let mut body_error = None;
        let mut retain_raw = raw.unwrap_or(false);
        if !source.is_empty() {
            let context = SingleInputContext {
                generation: &self.generation,
                request: &self.request,
                tx: &self.tx,
                group_scratch: &mut self.group_scratch,
                boolean_scratch: &mut self.boolean_scratch,
                capture_scratch: &mut self.capture_scratch,
                scan_cache: None,
                json_budget: None,
            };
            let mut collector = JsonCollector::new(
                self.generation.clone(),
                context,
                self.limits,
                self.evaluation_mode,
                limits.results,
            )
            .map_err(map_collector_error)?;
            let mut checker = JsonChecker::with_source_len("json", limits.lexical, limits.structure, source.len())
                .map_err(|kind| RuntimeError::JsonChecker { kind })?;
            let parsed = checker
                .feed(JsonStreamInput::Data(&source), &mut collector)
                .and_then(|()| checker.feed(JsonStreamInput::Complete, &mut collector));
            #[cfg(test)]
            measurements::record(Event::LexicalPeakBytes, checker.lexical_usage().peak_retained_bytes);
            #[cfg(test)]
            measurements::record(Event::StructurePeakBytes, checker.usage().peak_retained_bytes);
            #[cfg(test)]
            measurements::record(Event::LexicalWork, checker.lexical_usage().work);
            #[cfg(test)]
            measurements::record(Event::StructureWork, checker.usage().work);
            match parsed {
                Ok(()) => {
                    let (sealed, inspection_budget) = collector.into_sealed().map_err(map_collector_error)?;
                    accounting
                        .apply_json_summary(sealed.summary)
                        .map_err(|kind| RuntimeError::Body { kind })?;
                    results = Some(sealed);
                    budget = inspection_budget;
                }
                Err(error) => {
                    if let Some(error) = collector.failure() {
                        return Err(map_collector_error(error));
                    }
                    if !malformed(error) {
                        return Err(RuntimeError::JsonChecker { kind: error });
                    }
                    let kind = BodyError::Malformed {
                        processor: BodyProcessorSelection::Json,
                    };
                    if self.rule_engine_mode != RuleEngineMode::DetectionOnly {
                        return Err(RuntimeError::Body { kind });
                    }
                    budget = collector.into_abandoned_budget().map_err(map_collector_error)?;
                    body_error = Some(kind);
                    retain_raw = raw.unwrap_or(true);
                    accounting = BodyAccounting::json_prelude(source.len(), self.limits.body, retain_raw)
                        .map_err(|kind| RuntimeError::Body { kind })?;
                }
            }
        } else {
            budget = InspectionBudget::empty_publication(limits.results).map_err(map_store_error)?;
            accounting
                .apply_json_summary(JsonSemanticSummary::default())
                .map_err(|kind| RuntimeError::Body { kind })?;
        }
        let summary = accounting
            .finish_json(head_combined)
            .map_err(|kind| RuntimeError::Body { kind })?;
        let (length, length_start) = decimal_bytes(source.len());
        let (combined, combined_start) = decimal_bytes(summary.args_combined_size);
        // Inline arrays are included in the owner's initialization charge; each
        // used decimal digit is written once in addition to that initialization.
        budget
            .work(
                (length.len() - length_start)
                    .checked_add(combined.len() - combined_start)
                    .ok_or(RuntimeError::Arithmetic)?,
            )
            .map_err(map_store_error)?;
        let mut publication = Vec::new();
        budget
            .reserve_publication(&mut publication, 1)
            .map_err(map_store_error)?;
        publication.push(JsonPublication {
            results,
            source: retain_raw.then_some(source),
            length: (
                length,
                u8::try_from(length_start).map_err(|_| RuntimeError::Arithmetic)?,
            ),
            combined: (
                combined,
                u8::try_from(combined_start).map_err(|_| RuntimeError::Arithmetic)?,
            ),
            summary,
        });
        self.publish_body_state(RequestBodyPublication::Json(publication), body_error)?;
        self.json_budget = Some(budget);
        Ok(())
    }
}

/// This token can only be constructed for a JSON candidate selected by the
/// canonical target loop. Its compiled member/view determines fact applicability.
struct SelectedJsonCandidate {
    id: CandidateId,
    group: usize,
    collection: CollectionKind,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn evaluate_json_selected(
    context: &mut SingleInputContext<'_>,
    plan: SingleInputPlan<'_>,
    member: usize,
    id: CandidateId,
    collection: CollectionKind,
    work: &mut usize,
    detector_work: &mut usize,
    matched_count: usize,
) -> Result<Option<MemberMatch>, RuntimeError> {
    use super::super::json_inspection_plan::JsonMemberMode;
    let request = context.request;
    let results = request
        .body
        .as_ref()
        .and_then(RequestBodyPublication::json)
        .ok_or(RuntimeError::JsonInspectionState)?
        .results()?;
    let budget = context
        .json_budget
        .as_deref_mut()
        .ok_or(RuntimeError::JsonInspectionState)?;
    let candidate = results.candidate(budget, id).map_err(map_store_error)?;
    let mode = context
        .generation
        .json_inspection
        .members
        .get(member)
        .ok_or(RuntimeError::JsonInspectionState)?;
    let mode = if collection == CollectionKind::ArgsNames {
        mode.name
    } else {
        mode.value
    };
    let name = candidate.name;
    let make_matched = |value: &[u8], _: Option<&mut InspectionBudget>| {
        Ok((owned_copy(value)?, matched_variable_name(collection, Some(name))?))
    };
    match mode {
        JsonMemberMode::StaticFact(group) => replay_static(
            results,
            budget,
            context.generation,
            &plan,
            SelectedJsonCandidate { id, group, collection },
            work,
            detector_work,
            matched_count,
        ),
        JsonMemberMode::NameDynamic => {
            evaluate_single_input(context, plan, name, work, detector_work, matched_count, make_matched)
        }
        JsonMemberMode::NamedDeferred => evaluate_single_input(
            context,
            plan,
            candidate.deferred.ok_or(RuntimeError::JsonInspectionState)?,
            work,
            detector_work,
            matched_count,
            make_matched,
        ),
        JsonMemberMode::SemanticLength => {
            let (digits, start) = decimal_bytes(candidate.semantic_len);
            validate_length_recipe(
                candidate.semantic_len,
                &digits[start..],
                plan.transformations,
                plan.limits,
            )?;
            evaluate_transformed_input(
                context,
                &plan,
                &digits[start..],
                candidate.semantic_len,
                work,
                detector_work,
                matched_count,
                make_matched,
            )
        }
        _ => Err(RuntimeError::JsonInspectionState),
    }
}
fn validate_length_recipe(
    input: usize,
    decimal: &[u8],
    transformations: &[crate::seclang::TransformationKind],
    limits: RuntimeLimits,
) -> Result<(), RuntimeError> {
    use crate::seclang::TransformationKind;
    if !matches!(
        transformations,
        [TransformationKind::Length] | [TransformationKind::None, TransformationKind::Length]
    ) {
        return Err(RuntimeError::JsonInspectionState);
    }
    let bound = limits.max_transformation_output_bytes;
    if input > bound {
        return Err(map_transform_error(TransformError::OutputLimit { limit: bound }));
    }
    let mut work = 0usize;
    for transform in transformations {
        work = work.checked_add(input).ok_or(RuntimeError::Arithmetic)?;
        if work > limits.max_transformation_work_bytes {
            return Err(map_transform_error(TransformError::WorkLimit {
                limit: limits.max_transformation_work_bytes,
            }));
        }
        let output = if *transform == TransformationKind::Length {
            decimal.len()
        } else {
            input
        };
        if output > bound {
            return Err(map_transform_error(TransformError::OutputLimit { limit: bound }));
        }
        work = work.checked_add(output).ok_or(RuntimeError::Arithmetic)?;
        if work > limits.max_transformation_work_bytes {
            return Err(map_transform_error(TransformError::WorkLimit {
                limit: limits.max_transformation_work_bytes,
            }));
        }
    }
    Ok(())
}
#[allow(clippy::too_many_arguments)]
fn replay_static(
    results: &SealedResults<'_>,
    budget: &mut InspectionBudget,
    generation: &CompiledGeneration,
    plan: &SingleInputPlan<'_>,
    selected: SelectedJsonCandidate,
    work: &mut usize,
    detector_work: &mut usize,
    matched_count: usize,
) -> Result<Option<MemberMatch>, RuntimeError> {
    use super::json_inspection::{OutcomeValue, SelectedOutcome};
    let outcome = results
        .lookup_selected(
            budget,
            selected.id,
            selected.group,
            generation,
            plan.limits,
            plan.evaluation_mode,
        )
        .map_err(map_store_error)?;
    if let SelectedOutcome::TransformError(error) = outcome {
        return Err(error.error);
    }
    let candidate = results.candidate(budget, selected.id).map_err(map_store_error)?;
    let original_len = if selected.collection == CollectionKind::ArgsNames {
        candidate.name.len()
    } else {
        candidate.semantic_len
    };
    let group = generation
        .json_inspection
        .groups
        .get(selected.group)
        .ok_or(RuntimeError::JsonInspectionState)?;
    let profile = results
        .profile(budget, selected.id, group.profile)
        .map_err(map_store_error)?;
    let stored = match outcome {
        SelectedOutcome::MatchOrError(id) => {
            Some((id, results.outcome(budget, selected.id, id).map_err(map_store_error)?))
        }
        _ => None,
    };
    let mut next = profile.first_change;
    let mut stage_len = original_len;
    if !group.multi_match {
        while let Some(change) = next {
            let change = results
                .change(
                    budget,
                    selected.id,
                    profile.record.ok_or(RuntimeError::JsonInspectionState)?,
                    change,
                )
                .map_err(map_store_error)?;
            stage_len = change.output_len;
            next = change.next;
        }
    }
    let mut ordinal = 0usize;
    loop {
        let cost = detector_work_for_stage(plan.operator, stage_len)?;
        OrderedStageAccounting {
            original_bytes: original_len,
            limits: plan.limits,
            work,
            detector_work_bytes: detector_work,
        }
        .charge_stage(stage_len, cost)?;
        if let Some((id, outcome)) = &stored {
            if outcome.stage == ordinal {
                match &outcome.result {
                    OutcomeValue::Error(error) => return Err(error.error),
                    OutcomeValue::Match { value, first_capture } => {
                        let mut captures = Vec::new();
                        let mut next = *first_capture;
                        while let Some(capture) = next {
                            let capture = results
                                .capture(budget, selected.id, *id, capture)
                                .map_err(map_store_error)?;
                            captures.try_reserve(1).map_err(|_| RuntimeError::Allocation)?;
                            captures.push(CaptureEntry {
                                key: MatchValue::Json(capture.key),
                                value: MatchValue::Json(capture.value),
                            });
                            next = capture.next;
                        }
                        let name = MatchValue::Json(
                            results
                                .qualified_name(budget, selected.id, selected.collection)
                                .map_err(map_store_error)?,
                        );
                        return finish_member_match(
                            captures,
                            MatchValue::Json(*value),
                            name,
                            matched_count,
                            work,
                            plan.limits,
                        )
                        .map(Some);
                    }
                }
            }
        }
        if !group.multi_match {
            break;
        }
        let Some(change) = next else {
            break;
        };
        let change = results
            .change(
                budget,
                selected.id,
                profile.record.ok_or(RuntimeError::JsonInspectionState)?,
                change,
            )
            .map_err(map_store_error)?;
        next = change.next;
        stage_len = change.output_len;
        ordinal = ordinal.checked_add(1).ok_or(RuntimeError::Arithmetic)?;
    }
    if stored.is_some() {
        return Err(RuntimeError::JsonInspectionState);
    }
    Ok(None)
}

#[cfg(test)]
#[path = "../../tests/unit/seclang/json_inspection_runtime_test.rs"]
mod tests;
