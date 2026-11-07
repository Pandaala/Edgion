//! Scoped XML source ownership and atomic publication; predicate facts use the shared engine.
use super::json_inspection_collector::{CollectorError, JsonCollector, JsonCollectorState};
use super::*;
use crate::seclang::body::BodyAccounting;
use crate::seclang::{
    BodyProcessorSelection, XmlPublicationSummary, XmlStream, XmlStreamError, XmlStreamInput, XmlStreamLimit,
    XmlStreamLimits, XmlStreamUsage,
};
use bytes::Bytes;

#[derive(Debug, Clone, Copy, Default)]
pub struct XmlInspectionLimits {
    pub parser: XmlStreamLimits,
    pub results: JsonResultLimits,
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct XmlInspectionUsage {
    pub parser: XmlStreamUsage,
    pub result_heap_bytes: usize,
    pub result_peak_heap_bytes: usize,
    pub result_work: usize,
}
impl XmlInspectionUsage {
    pub fn current_heap_bytes(self) -> Option<usize> {
        self.parser.retained_bytes.checked_add(self.result_heap_bytes)
    }
}
pub(super) struct XmlPublication<'g> {
    pub results: Option<SealedResults<'g>>,
    pub source: Option<Bytes>,
    pub summary: XmlPublicationSummary,
    length: ([u8; 40], u8),
    combined: ([u8; 40], u8),
}
impl XmlPublication<'_> {
    pub(super) fn metadata_value(&self, collection: CollectionKind) -> Option<&[u8]> {
        match collection {
            CollectionKind::RequestBody => self.source.as_deref(),
            CollectionKind::RequestBodyLength => Some(&self.length.0[usize::from(self.length.1)..]),
            CollectionKind::ReqbodyProcessor => Some(b"XML"),
            CollectionKind::ArgsCombinedSize => Some(&self.combined.0[usize::from(self.combined.1)..]),
            _ => None,
        }
    }
}
pub(super) fn xml_error(error: RuntimeError) -> RuntimeError {
    match error {
        RuntimeError::JsonInspectionState => RuntimeError::XmlInspectionState,
        RuntimeError::JsonResultLimit { kind } => RuntimeError::XmlResultLimit { kind },
        _ => error,
    }
}
fn collector_error(error: CollectorError) -> RuntimeError {
    xml_error(match error {
        CollectorError::Store(error) => map_store_error(error),
        CollectorError::Runtime(error) => error,
        CollectorError::State => RuntimeError::JsonInspectionState,
    })
}
fn malformed(error: XmlStreamError) -> bool {
    matches!(
        error,
        XmlStreamError::Syntax | XmlStreamError::Character | XmlStreamError::Incomplete
    )
}
pub struct XmlInspectionSession<'call, 'generation> {
    transaction: &'call mut Transaction<'generation>,
    limits: XmlInspectionLimits,
    source_hint: Option<usize>,
    observed: usize,
    complete_len: Option<usize>,
    completion_attempted: bool,
    parser: Option<XmlStream>,
    collector: Option<JsonCollectorState<'generation>>,
    parse_error: Option<XmlStreamError>,
    failure: Option<RuntimeError>,
    accounting: Option<BodyAccounting>,
    finished: bool,
}
impl<'generation> Transaction<'generation> {
    pub fn start_xml_inspection(
        &mut self,
        limits: XmlInspectionLimits,
        source_hint: Option<usize>,
    ) -> Result<XmlInspectionSession<'_, 'generation>, RuntimeError> {
        if self.preflight_request_body(BodyProcessorSelection::Xml)? != BodyProcessorSelection::Xml {
            return Err(RuntimeError::XmlInspectionState);
        }
        if self.request_body_stream.is_some() || self.request_prefix_completion.is_some() {
            return Err(RuntimeError::RequestBodyAlreadyPublished);
        }
        if let Some(finding) = self.generation.xml_inspection.unsupported.first() {
            let error = RuntimeError::XmlUnsupported {
                rule_id: finding.rule_id,
            };
            self.json_failure.get_or_insert(error);
            return Err(error);
        }
        if let Some(finding) = self.generation.xml_inspection.xml_unsupported.first() {
            let error = RuntimeError::XmlUnsupported {
                rule_id: finding.rule_id,
            };
            self.json_failure.get_or_insert(error);
            return Err(error);
        }
        Ok(XmlInspectionSession {
            transaction: self,
            limits,
            source_hint,
            observed: 0,
            complete_len: None,
            completion_attempted: false,
            parser: None,
            collector: None,
            parse_error: None,
            failure: None,
            accounting: None,
            finished: false,
        })
    }
    pub fn publish_request_body_with_xml_limits(
        &mut self,
        body: RequestBody,
        limits: XmlInspectionLimits,
    ) -> Result<(), RuntimeError> {
        let result = self.publish_xml_inner(body, limits);
        if let Err(error) = result {
            self.json_failure.get_or_insert(error);
        }
        result
    }
    fn publish_xml_inner(&mut self, body: RequestBody, limits: XmlInspectionLimits) -> Result<(), RuntimeError> {
        let (source, requested) = body.into_parts();
        if self.preflight_request_body(requested)? != BodyProcessorSelection::Xml {
            return self.publish_request_body(RequestBody::new(source, requested));
        }
        let mut inspection = self.start_xml_inspection(limits, Some(source.len()))?;
        let _ = inspection.feed(&source);
        let completed = inspection.complete(source.len());
        let raw = inspection.requires_raw_source().then_some(source);
        completed?;
        inspection.finish(raw)
    }
}
impl XmlInspectionSession<'_, '_> {
    pub fn usage(&self) -> XmlInspectionUsage {
        let results = self
            .collector
            .as_ref()
            .map(JsonCollectorState::usage)
            .unwrap_or_default();
        XmlInspectionUsage {
            parser: self.parser.as_ref().map(XmlStream::usage).unwrap_or_default(),
            result_heap_bytes: results.retained_bytes,
            result_peak_heap_bytes: results.peak_retained_bytes,
            result_work: results.work,
        }
    }
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
            return Err(RuntimeError::XmlInspectionState);
        }
        self.observed = match self.observed.checked_add(data.len()) {
            Some(value) => value,
            None => {
                self.failure = Some(RuntimeError::Arithmetic);
                return Err(RuntimeError::Arithmetic);
            }
        };
        if self.observed > self.limits.parser.max_source_bytes {
            let error = RuntimeError::XmlStream {
                kind: XmlStreamError::Limit {
                    kind: XmlStreamLimit::SourceBytes,
                    limit: self.limits.parser.max_source_bytes,
                },
            };
            self.failure = Some(error);
            return Err(error);
        }
        if data.is_empty() || self.parse_error.is_some() {
            return Ok(());
        }
        let result = self.parse(XmlStreamInput::Data(data));
        if let Err(error) = result {
            self.failure.get_or_insert(error);
        }
        result
    }
    fn parse(&mut self, input: XmlStreamInput<'_>) -> Result<(), RuntimeError> {
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
            transform_memo: None,
        };
        let mut collector = match self.collector.take() {
            Some(state) => JsonCollector::resume(state, context),
            None => JsonCollector::new_domain(
                tx.generation.clone(),
                context,
                tx.limits,
                tx.evaluation_mode,
                self.limits.results,
                InspectionDomain::Xml,
            ),
        }
        .map_err(collector_error)?;
        let parser = self.parser.get_or_insert_with(|| XmlStream::new(self.limits.parser));
        let parsed = parser.feed(input, &mut collector);
        let failure = collector.failure();
        if let Err(error) = parsed {
            if malformed(error) {
                collector.abandon_xml();
            }
        }
        self.collector = Some(collector.into_state());
        if let Some(error) = failure {
            return Err(collector_error(error));
        }
        match parsed {
            Ok(()) => Ok(()),
            Err(error) if malformed(error) => {
                self.parse_error = Some(error);
                if tx.rule_engine_mode == RuleEngineMode::DetectionOnly {
                    Ok(())
                } else {
                    Err(RuntimeError::Body {
                        kind: BodyError::Malformed {
                            processor: BodyProcessorSelection::Xml,
                        },
                    })
                }
            }
            Err(kind) => Err(RuntimeError::XmlStream { kind }),
        }
    }
    pub fn complete(&mut self, len: usize) -> Result<(), RuntimeError> {
        if self.completion_attempted {
            return Err(self.failure.unwrap_or(RuntimeError::XmlInspectionState));
        }
        self.completion_attempted = true;
        let result = (|| {
            let raw = self
                .transaction
                .tx
                .force_request_body_variable
                .map(|value| value == super::super::OnOff::On);
            let mut accounting = BodyAccounting::structured_prelude(
                len,
                self.transaction.limits.body,
                raw.unwrap_or(false),
                BodyProcessorSelection::Xml,
            )
            .map_err(|kind| RuntimeError::Body { kind })?;
            if let Some(error) = self.failure {
                return Err(error);
            }
            if len != self.observed || self.source_hint.is_some_and(|hint| hint != len) {
                return Err(RuntimeError::XmlInspectionState);
            }
            if len > self.limits.parser.max_source_bytes {
                return Err(RuntimeError::XmlStream {
                    kind: XmlStreamError::Limit {
                        kind: XmlStreamLimit::SourceBytes,
                        limit: self.limits.parser.max_source_bytes,
                    },
                });
            }
            if self.parser.is_some() && self.parse_error.is_none() {
                self.parse(XmlStreamInput::Complete)?;
            }
            if self.parse_error.is_some() {
                accounting = BodyAccounting::structured_prelude(
                    len,
                    self.transaction.limits.body,
                    raw.unwrap_or(true),
                    BodyProcessorSelection::Xml,
                )
                .map_err(|kind| RuntimeError::Body { kind })?;
            } else if let Some(parser) = &self.parser {
                accounting
                    .apply_xml_summary(parser.summary())
                    .map_err(|kind| RuntimeError::Body { kind })?;
            }
            self.complete_len = Some(len);
            self.accounting = Some(accounting);
            Ok(())
        })();
        if let Err(error) = result {
            self.failure = Some(error);
            self.transaction.json_failure.get_or_insert(error);
        }
        result
    }
    pub fn requires_raw_source(&self) -> bool {
        self.transaction.tx.force_request_body_variable.map_or(
            self.parse_error.is_some() && self.transaction.rule_engine_mode == RuleEngineMode::DetectionOnly,
            |value| value == super::super::OnOff::On,
        )
    }
    /// End an explicitly bounded inspection without fabricating EOF or publishing
    /// provisional body facts. Genuine errors retain precedence over policy stop.
    pub fn stop_by_policy(mut self) -> Result<(), RuntimeError> {
        let error = self.failure.or_else(|| {
            self.parse_error.map(|_| RuntimeError::Body {
                kind: BodyError::Malformed {
                    processor: BodyProcessorSelection::Xml,
                },
            })
        });
        self.finished = true;
        if let Some(error) = error {
            self.transaction.json_failure.get_or_insert(error);
            return Err(error);
        }
        Ok(())
    }

    pub fn finish(mut self, source: Option<Bytes>) -> Result<(), RuntimeError> {
        let result = self.finish_inner(source);
        self.finished = true;
        if let Err(error) = result {
            self.transaction.json_failure.get_or_insert(error);
        }
        result
    }
    fn finish_inner(&mut self, source: Option<Bytes>) -> Result<(), RuntimeError> {
        if let Some(error) = self.failure {
            return Err(error);
        }
        let len = self.complete_len.ok_or(RuntimeError::XmlInspectionState)?;
        if source.as_ref().is_some_and(|source| source.len() != len) || (self.requires_raw_source() && source.is_none())
        {
            return Err(RuntimeError::XmlInspectionState);
        }
        let head = parse_usize(
            self.transaction
                .request
                .first_value(CollectionKind::ArgsCombinedSize)
                .ok_or(RuntimeError::GenerationInvariant)?,
        )?;
        let summary = self
            .accounting
            .take()
            .ok_or(RuntimeError::XmlInspectionState)?
            .finish_xml(head)
            .map_err(|kind| RuntimeError::Body { kind })?;
        let (results, mut budget) = match self.collector.take() {
            Some(state) if self.parse_error.is_some() => {
                (None, state.into_abandoned_budget().map_err(collector_error)?)
            }
            Some(state) => {
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
                    transform_memo: None,
                };
                let mut collector = JsonCollector::resume(state, context).map_err(collector_error)?;
                collector
                    .seal_xml(self.parser.as_ref().ok_or(RuntimeError::XmlInspectionState)?.summary())
                    .map_err(collector_error)?;
                let (results, budget) = collector.into_sealed().map_err(collector_error)?;
                (Some(results), budget)
            }
            None => (
                None,
                InspectionBudget::empty_publication(self.limits.results)
                    .map_err(map_store_error)
                    .map_err(xml_error)?,
            ),
        };

        let (length, start) = decimal_bytes(len);
        let (combined, cstart) = decimal_bytes(summary.args_combined_size);
        budget
            .work(
                (length.len() - start)
                    .checked_add(combined.len() - cstart)
                    .ok_or(RuntimeError::Arithmetic)?,
            )
            .map_err(map_store_error)
            .map_err(xml_error)?;
        let mut publication = Vec::new();
        budget
            .reserve_publication(&mut publication, 1)
            .map_err(map_store_error)
            .map_err(xml_error)?;
        publication.push(XmlPublication {
            results,
            source: if self.requires_raw_source() { source } else { None },
            summary,
            length: (length, u8::try_from(start).map_err(|_| RuntimeError::Arithmetic)?),
            combined: (combined, u8::try_from(cstart).map_err(|_| RuntimeError::Arithmetic)?),
        });
        let error = self.parse_error.map(|_| BodyError::Malformed {
            processor: BodyProcessorSelection::Xml,
        });
        self.transaction
            .publish_body_state(RequestBodyPublication::Xml(publication), error)?;
        self.transaction.json_budget = Some(budget);
        Ok(())
    }
}
impl Drop for XmlInspectionSession<'_, '_> {
    fn drop(&mut self) {
        if !self.finished {
            self.transaction
                .json_failure
                .get_or_insert(self.failure.unwrap_or(RuntimeError::XmlStream {
                    kind: XmlStreamError::Abandoned,
                }));
        }
    }
}

#[cfg(test)]
#[path = "../../tests/unit/seclang/xml_inspection_runtime_test.rs"]
mod tests;
