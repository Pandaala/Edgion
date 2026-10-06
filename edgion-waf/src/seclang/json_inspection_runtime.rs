//! Canonical JSON publication and ordered consumption of provisional results.
use super::json_inspection::{ResultLimits, SealedResults, StoreError};
use super::json_inspection_collector::{CollectorError, JsonCollector, JsonCollectorState};
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
        let budget;
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
        self.publish_json_inspection(
            source.len(),
            retain_raw.then_some(source),
            accounting,
            head_combined,
            results,
            budget,
            body_error,
        )
    }
    #[allow(clippy::too_many_arguments)]
    fn publish_json_inspection(
        &mut self,
        source_len: usize,
        source: Option<Bytes>,
        accounting: BodyAccounting,
        head_combined: usize,
        results: Option<SealedResults<'generation>>,
        mut budget: InspectionBudget,
        body_error: Option<BodyError>,
    ) -> Result<(), RuntimeError> {
        let summary = accounting
            .finish_json(head_combined)
            .map_err(|kind| RuntimeError::Body { kind })?;
        let (length, length_start) = decimal_bytes(source_len);
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
            source,
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

/// Allocation-capacity observations. Component peaks are independent, not RSS
/// or a simultaneous process high-water mark. Fixed inline storage is separate.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct JsonInspectionUsage {
    pub inspected_source_bytes: usize,
    pub lexical_heap_bytes: usize,
    pub structure_heap_bytes: usize,
    pub result_heap_bytes: usize,
    pub lexical_peak_heap_bytes: usize,
    pub structure_peak_heap_bytes: usize,
    pub result_peak_heap_bytes: usize,
    pub lexical_work: usize,
    pub structure_work: usize,
    pub result_work: usize,
}
impl JsonInspectionUsage {
    pub fn current_heap_bytes(self) -> Option<usize> {
        self.lexical_heap_bytes
            .checked_add(self.structure_heap_bytes)?
            .checked_add(self.result_heap_bytes)
    }
}

/// Scoped provisional JSON inspection. The mutable transaction borrow prevents
/// execution or publication until true completion; dropping this guard abandons it.
pub struct JsonInspectionSession<'call, 'generation> {
    transaction: &'call mut Transaction<'generation>,
    limits: JsonInspectionLimits,
    source_hint: Option<usize>,
    observed_bytes: usize,
    complete_len: Option<usize>,
    completion_attempted: bool,
    checker: Option<JsonChecker<super::json_inspection::Receipt>>,
    collector: Option<JsonCollectorState<'generation>>,
    parse_error: Option<JsonCheckerError>,
    failure: Option<RuntimeError>,
    accounting: Option<BodyAccounting>,
    finished: bool,
}
impl<'generation> Transaction<'generation> {
    /// Begin JSON inspection after request-header controls. No parser storage is
    /// allocated until the first nonempty chunk. Complete-source callers may hint
    /// their exact size; transport callers leave it unknown.
    pub fn start_json_inspection(
        &mut self,
        limits: JsonInspectionLimits,
        source_len_hint: Option<usize>,
    ) -> Result<JsonInspectionSession<'_, 'generation>, RuntimeError> {
        if self.preflight_request_body(BodyProcessorSelection::Json)? != BodyProcessorSelection::Json {
            return Err(RuntimeError::JsonInspectionState);
        }
        if self.request_body_stream.is_some() || self.request_prefix_completion.is_some() {
            return Err(RuntimeError::RequestBodyAlreadyPublished);
        }
        if let Some(unsupported) = self.generation.json_inspection.unsupported.first() {
            return Err(RuntimeError::JsonUnsupported {
                rule_id: unsupported.rule_id,
            });
        }
        Ok(JsonInspectionSession {
            transaction: self,
            limits,
            source_hint: source_len_hint,
            observed_bytes: 0,
            complete_len: None,
            completion_attempted: false,
            checker: None,
            collector: None,
            parse_error: None,
            failure: None,
            accounting: None,
            finished: false,
        })
    }
}
impl JsonInspectionSession<'_, '_> {
    pub fn usage(&self) -> JsonInspectionUsage {
        let lexical = self
            .checker
            .as_ref()
            .map(|checker| checker.lexical_usage())
            .unwrap_or_default();
        let structure = self.checker.as_ref().map(|checker| checker.usage()).unwrap_or_default();
        let result = self
            .collector
            .as_ref()
            .map(|collector| collector.usage())
            .unwrap_or_default();
        JsonInspectionUsage {
            inspected_source_bytes: lexical.source_bytes,
            lexical_heap_bytes: lexical.retained_bytes,
            structure_heap_bytes: structure.retained_bytes,
            result_heap_bytes: result.retained_bytes,
            lexical_peak_heap_bytes: lexical.peak_retained_bytes,
            structure_peak_heap_bytes: structure.peak_retained_bytes,
            result_peak_heap_bytes: result.peak_retained_bytes,
            lexical_work: lexical.work,
            structure_work: structure.work,
            result_work: result.work,
        }
    }
    /// Existing installed accounting is logical; provisional parser/store usage
    /// is additional admitted capacity, not another copy of the request source.
    pub fn runtime_bytes(&self) -> Option<usize> {
        self.transaction
            .retained_bytes()
            .checked_add(self.usage().current_heap_bytes()?)
    }
    pub fn feed(&mut self, data: &[u8]) -> Result<(), RuntimeError> {
        if let Some(error) = self.failure {
            return Err(error);
        }
        if self.completion_attempted {
            return Err(RuntimeError::JsonInspectionState);
        }
        let Some(observed) = self.observed_bytes.checked_add(data.len()) else {
            self.failure = Some(RuntimeError::Arithmetic);
            return Err(RuntimeError::Arithmetic);
        };
        self.observed_bytes = observed;
        if data.is_empty() || self.parse_error.is_some() {
            return Ok(());
        }
        let result = self.feed_checker(JsonStreamInput::Data(data));
        if let Err(error) = result {
            self.failure.get_or_insert(error);
        }
        result
    }
    fn feed_checker(&mut self, input: JsonStreamInput<'_>) -> Result<(), RuntimeError> {
        let tx = &mut self.transaction;
        let context = SingleInputContext {
            generation: &tx.generation,
            request: &tx.request,
            tx: &tx.tx,
            group_scratch: &mut tx.group_scratch,
            boolean_scratch: &mut tx.boolean_scratch,
            capture_scratch: &mut tx.capture_scratch,
            scan_cache: None,
            json_budget: None,
        };
        let mut collector = match self.collector.take() {
            Some(state) => JsonCollector::resume(state, context),
            None => JsonCollector::new(
                tx.generation.clone(),
                context,
                tx.limits,
                tx.evaluation_mode,
                self.limits.results,
            ),
        }
        .map_err(map_collector_error)?;
        if self.checker.is_none() {
            self.checker = Some(
                match self.source_hint {
                    Some(len) => JsonChecker::with_source_len("json", self.limits.lexical, self.limits.structure, len),
                    None => JsonChecker::new("json", self.limits.lexical, self.limits.structure),
                }
                .map_err(|kind| RuntimeError::JsonChecker { kind })?,
            );
        }
        let parsed = self
            .checker
            .as_mut()
            .ok_or(RuntimeError::JsonInspectionState)?
            .feed(input, &mut collector);
        let collector_failure = collector.failure();
        self.collector = Some(collector.into_state());
        if let Some(error) = collector_failure {
            return Err(map_collector_error(error));
        }
        if let Err(kind) = parsed {
            if malformed(kind) {
                self.parse_error = Some(kind);
                if tx.rule_engine_mode == RuleEngineMode::DetectionOnly {
                    return Ok(());
                }
                return Err(RuntimeError::Body {
                    kind: BodyError::Malformed {
                        processor: BodyProcessorSelection::Json,
                    },
                });
            }
            return Err(RuntimeError::JsonChecker { kind });
        }
        Ok(())
    }
    /// Called only after transport/snapshot EOF. The complete source prelude
    /// retains priority over any latched parser failure. Captured length is
    /// independent of the lexer's admitted bytes after a failure.
    pub fn complete(&mut self, captured_len: usize) -> Result<(), RuntimeError> {
        if self.completion_attempted {
            return Err(self.failure.unwrap_or(RuntimeError::JsonInspectionState));
        }
        self.completion_attempted = true;
        let raw = self
            .transaction
            .tx
            .force_request_body_variable
            .map(|value| value == super::super::OnOff::On);
        let result = (|| {
            let accounting =
                BodyAccounting::json_prelude(captured_len, self.transaction.limits.body, raw.unwrap_or(false))
                    .map_err(|kind| RuntimeError::Body { kind })?;
            if let Some(error) = self.failure {
                return Err(error);
            }
            if captured_len != self.observed_bytes || self.source_hint.is_some_and(|hint| hint != captured_len) {
                return Err(RuntimeError::JsonInspectionState);
            }
            if self.checker.is_some() && self.parse_error.is_none() {
                self.feed_checker(JsonStreamInput::Complete)?;
            }
            self.accounting = Some(if self.parse_error.is_some() {
                BodyAccounting::json_prelude(captured_len, self.transaction.limits.body, raw.unwrap_or(true))
                    .map_err(|kind| RuntimeError::Body { kind })?
            } else {
                accounting
            });
            self.complete_len = Some(captured_len);
            Ok(())
        })();
        if let Err(error) = result {
            self.failure = Some(error);
            self.transaction.json_failure.get_or_insert(error);
        }
        result
    }
    /// Raw targets do not override JSON visibility. Query after complete so only
    /// actual detection-only syntax fallback requires its default raw source.
    pub fn requires_raw_source(&self) -> bool {
        self.transaction.tx.force_request_body_variable.map_or(
            self.parse_error.is_some() && self.transaction.rule_engine_mode == RuleEngineMode::DetectionOnly,
            |value| value == super::super::OnOff::On,
        )
    }
    /// Publish once after completion, borrowing no bytes from supplied chunks.
    pub fn finish(mut self, source: Option<Bytes>) -> Result<(), RuntimeError> {
        let result = self.finish_inner(source);
        if let Err(error) = result {
            self.transaction.json_failure.get_or_insert(error);
        }
        self.finished = true;
        result
    }
    fn finish_inner(&mut self, source: Option<Bytes>) -> Result<(), RuntimeError> {
        if let Some(error) = self.failure {
            return Err(error);
        }
        let len = self.complete_len.ok_or(RuntimeError::JsonInspectionState)?;
        if source.as_ref().is_some_and(|source| source.len() != len) || (self.requires_raw_source() && source.is_none())
        {
            return Err(RuntimeError::JsonInspectionState);
        }
        let (results, budget, body_error) = match self.collector.take() {
            Some(state) if self.parse_error.is_some() => (
                None,
                state.into_abandoned_budget().map_err(map_collector_error)?,
                Some(BodyError::Malformed {
                    processor: BodyProcessorSelection::Json,
                }),
            ),
            Some(state) => {
                let (sealed, budget) = state.into_sealed().map_err(map_collector_error)?;
                (Some(sealed), budget, None)
            }
            None => (
                None,
                InspectionBudget::empty_publication(self.limits.results).map_err(map_store_error)?,
                None,
            ),
        };
        let mut accounting = self.accounting.take().ok_or(RuntimeError::JsonInspectionState)?;
        if let Some(results) = &results {
            accounting
                .apply_json_summary(results.summary)
                .map_err(|kind| RuntimeError::Body { kind })?;
        } else if body_error.is_none() {
            accounting
                .apply_json_summary(JsonSemanticSummary::default())
                .map_err(|kind| RuntimeError::Body { kind })?;
        }
        let head = parse_usize(
            self.transaction
                .request
                .first_value(CollectionKind::ArgsCombinedSize)
                .ok_or(RuntimeError::GenerationInvariant)?,
        )?;
        let source = if self.requires_raw_source() { source } else { None };
        self.transaction
            .publish_json_inspection(len, source, accounting, head, results, budget, body_error)
    }
}
impl Drop for JsonInspectionSession<'_, '_> {
    fn drop(&mut self) {
        if !self.finished {
            self.transaction
                .json_failure
                .get_or_insert(self.failure.unwrap_or(RuntimeError::JsonChecker {
                    kind: JsonCheckerError::Lexical(JsonStreamError::Abandoned),
                }));
        }
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
