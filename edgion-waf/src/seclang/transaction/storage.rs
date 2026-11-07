use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ByteRange {
    pub(super) start: usize,
    pub(super) len: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CollectionValue {
    pub(super) name: Option<ByteRange>,
    pub(super) value: ByteRange,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SelectedValue {
    pub(super) collection: CollectionKind,
    pub(super) name: Option<SelectedName>,
    pub(super) value: SelectedBytes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SelectedBytes {
    Request(ByteRange),
    Body(usize),
    Response(usize),
    ResponseWindow,
    Tx(usize),
    MatchedVar,
    MatchedVars(usize),
    CtlProcessor,
    Json(CandidateId),
    XmlText(CandidateId),
    XmlAttribute(CandidateId),
    XmlMetadata,
    JsonMetadata,
    Count { digits: [u8; 40], start: u8 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SelectedName {
    Json(CandidateId),
    Xml(CandidateId),
    Request(ByteRange),
    Body(usize),
    Response(usize),
    Tx(usize),
    MatchedVars(usize),
}

impl SelectedValue {
    pub(super) fn bytes<'value>(
        &'value self,
        request: &'value RequestState,
        tx: &'value TxState,
        response: Option<&'value ResponseState>,
        json_budget: Option<&mut InspectionBudget>,
    ) -> Result<&'value [u8], RuntimeError> {
        match &self.value {
            SelectedBytes::Json(id) => {
                let body = request
                    .body
                    .as_ref()
                    .and_then(RequestBodyPublication::json)
                    .ok_or(RuntimeError::JsonInspectionState)?;
                let candidate = body
                    .results()?
                    .candidate(json_budget.ok_or(RuntimeError::JsonInspectionState)?, *id)
                    .map_err(map_store_error)?;
                if self.collection == CollectionKind::ArgsNames {
                    Ok(candidate.name)
                } else {
                    candidate.deferred.ok_or(RuntimeError::JsonInspectionState)
                }
            }
            SelectedBytes::XmlText(_) | SelectedBytes::XmlAttribute(_) => Err(RuntimeError::XmlInspectionState),
            SelectedBytes::XmlMetadata => request
                .body
                .as_ref()
                .and_then(RequestBodyPublication::xml)
                .and_then(|body| body.metadata_value(self.collection))
                .ok_or(RuntimeError::XmlInspectionState),
            SelectedBytes::JsonMetadata => request
                .body
                .as_ref()
                .and_then(RequestBodyPublication::json)
                .and_then(|body| body.metadata_value(self.collection))
                .ok_or(RuntimeError::JsonInspectionState),
            SelectedBytes::Request(range) => Ok(request.bytes_at(*range)),
            SelectedBytes::Body(index) => request
                .body
                .as_ref()
                .and_then(RequestBodyPublication::materialized)
                .and_then(|body| body.entries.get(*index))
                .map(|entry| entry.value.as_slice())
                .ok_or(RuntimeError::GenerationInvariant),
            SelectedBytes::Response(index) => response
                .and_then(|response| response.entries.get(*index))
                .map(|entry| entry.value.as_slice())
                .ok_or(RuntimeError::GenerationInvariant),
            SelectedBytes::ResponseWindow => response
                .and_then(|response| response.window.as_deref())
                .ok_or(RuntimeError::GenerationInvariant),
            SelectedBytes::Tx(index) => tx
                .entries
                .get(*index)
                .ok_or(RuntimeError::GenerationInvariant)?
                .value
                .bytes(request, json_budget),
            SelectedBytes::MatchedVar => tx
                .matched_var
                .as_ref()
                .ok_or(RuntimeError::GenerationInvariant)?
                .bytes(request, json_budget),
            SelectedBytes::MatchedVars(index) => tx
                .matched_vars
                .get(*index)
                .ok_or(RuntimeError::GenerationInvariant)?
                .value
                .bytes(request, json_budget),
            SelectedBytes::CtlProcessor => tx
                .request_body_processor
                .map(super::super::super::BodyProcessorSelection::as_bytes)
                .ok_or(RuntimeError::GenerationInvariant),
            SelectedBytes::Count { digits, start } => Ok(&digits[usize::from(*start)..]),
        }
    }

    pub(super) fn name_bytes<'value>(
        self,
        request: &'value RequestState,
        tx: &'value TxState,
        response: Option<&'value ResponseState>,
        json_budget: Option<&mut InspectionBudget>,
    ) -> Result<Option<&'value [u8]>, RuntimeError> {
        selected_name_bytes(self.name, request, tx, response, json_budget)
    }
}

pub(super) fn inspection_error(request: &RequestState, error: RuntimeError) -> RuntimeError {
    if request
        .body
        .as_ref()
        .and_then(RequestBodyPublication::structured_results)
        .is_some_and(|results| results.domain() == InspectionDomain::Xml)
        || matches!(request.body, Some(RequestBodyPublication::Xml(_)))
    {
        xml_inspection_runtime::xml_error(error)
    } else {
        error
    }
}

pub(super) fn selected_name_bytes<'value>(
    name: Option<SelectedName>,
    request: &'value RequestState,
    tx: &'value TxState,
    response: Option<&'value ResponseState>,
    json_budget: Option<&mut InspectionBudget>,
) -> Result<Option<&'value [u8]>, RuntimeError> {
    name.map(|name| match name {
        SelectedName::Json(id) | SelectedName::Xml(id) => request
            .body
            .as_ref()
            .and_then(RequestBodyPublication::structured_results)
            .ok_or(RuntimeError::JsonInspectionState)?
            .candidate(json_budget.ok_or(RuntimeError::JsonInspectionState)?, id)
            .map(|candidate| candidate.name)
            .map_err(map_store_error),
        SelectedName::Request(range) => Ok(request.bytes_at(range)),
        SelectedName::Body(index) => request
            .body
            .as_ref()
            .and_then(RequestBodyPublication::materialized)
            .and_then(|body| body.entries.get(index))
            .and_then(|entry| entry.name.as_deref())
            .ok_or(RuntimeError::GenerationInvariant),
        SelectedName::Response(index) => response
            .and_then(|response| response.entries.get(index))
            .and_then(|entry| entry.name.as_deref())
            .ok_or(RuntimeError::GenerationInvariant),
        SelectedName::Tx(index) => tx
            .entries
            .get(index)
            .ok_or(RuntimeError::GenerationInvariant)?
            .key
            .bytes(request, json_budget),
        SelectedName::MatchedVars(index) => tx
            .matched_vars
            .get(index)
            .ok_or(RuntimeError::GenerationInvariant)?
            .key
            .bytes(request, json_budget),
    })
    .transpose()
    .map_err(|error| inspection_error(request, error))
}

pub(super) const COLLECTION_COUNT: usize = CollectionKind::ALL.len();

pub(super) enum RequestBodyPublication<'generation> {
    Json(Vec<JsonPublication<'generation>>),
    Xml(Vec<xml_inspection_runtime::XmlPublication<'generation>>),
    Materialized(BodyPublication),
}
impl RequestBodyPublication<'_> {
    pub(super) fn json(&self) -> Option<&JsonPublication<'_>> {
        match self {
            Self::Json(body) => body.first(),
            _ => None,
        }
    }
    pub(super) fn xml(&self) -> Option<&xml_inspection_runtime::XmlPublication<'_>> {
        match self {
            Self::Xml(body) => body.first(),
            _ => None,
        }
    }
    pub(super) fn structured_results(&self) -> Option<&SealedResults<'_>> {
        match self {
            Self::Json(body) => body.first().and_then(|body| body.results.as_ref()),
            Self::Xml(body) => body.first().and_then(|body| body.results.as_ref()),
            _ => None,
        }
    }
    pub(super) fn materialized(&self) -> Option<&BodyPublication> {
        match self {
            Self::Materialized(body) => Some(body),
            Self::Json(_) | Self::Xml(_) => None,
        }
    }
    pub(super) fn entry_count(&self) -> usize {
        match self {
            Self::Materialized(body) => body.entries.len(),
            Self::Json(body) => body.first().map_or(0, |body| body.summary.entries),
            Self::Xml(body) => body.first().map_or(0, |body| body.summary.entries),
        }
    }
    pub(super) fn retained_bytes(&self) -> usize {
        match self {
            Self::Materialized(body) => body.retained_bytes,
            Self::Json(body) => body.first().map_or(0, |body| body.summary.retained_bytes),
            Self::Xml(body) => body.first().map_or(0, |body| body.summary.retained_bytes),
        }
    }
}

pub(super) struct TxEntry<V = MatchValue, K = MatchValue> {
    pub(super) key: K,
    pub(super) value: V,
}

/// JSON handles refer only to the immutable request-owned result arena. Owned
/// values preserve the fallible ordinary matching path and logical byte charges.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum MatchValue {
    Owned(Vec<u8>),
    Json(SealedMatchValue),
}
impl From<Vec<u8>> for MatchValue {
    fn from(value: Vec<u8>) -> Self {
        Self::Owned(value)
    }
}
impl MatchValue {
    pub(super) fn len(&self) -> usize {
        match self {
            Self::Owned(value) => value.len(),
            Self::Json(value) => value.len(),
        }
    }
    pub(super) fn owned_capacity(&self) -> usize {
        match self {
            Self::Owned(value) => value.capacity(),
            Self::Json(_) => 0,
        }
    }
    /// Immutable observation only. Rule execution must use the metered seam.
    pub(super) fn observed_bytes<'value>(
        &'value self,
        request: &'value RequestState,
    ) -> Result<&'value [u8], RuntimeError> {
        match self {
            Self::Owned(value) => Ok(value),
            Self::Json(value) => request
                .body
                .as_ref()
                .and_then(RequestBodyPublication::structured_results)
                .ok_or_else(|| inspection_error(request, RuntimeError::JsonInspectionState))?
                .observe_match_value(*value)
                .map_err(|error| inspection_error(request, map_store_error(error))),
        }
    }
    pub(super) fn try_clone(&self) -> Result<Self, RuntimeError> {
        match self {
            Self::Owned(value) => owned_copy(value).map(Self::Owned),
            Self::Json(value) => Ok(Self::Json(*value)),
        }
    }
    pub(super) fn bytes<'value>(
        &'value self,
        request: &'value RequestState,
        budget: Option<&mut InspectionBudget>,
    ) -> Result<&'value [u8], RuntimeError> {
        match self {
            Self::Owned(value) => Ok(value),
            Self::Json(value) => request
                .body
                .as_ref()
                .and_then(RequestBodyPublication::structured_results)
                .ok_or_else(|| inspection_error(request, RuntimeError::JsonInspectionState))?
                .match_value(
                    budget.ok_or_else(|| inspection_error(request, RuntimeError::JsonInspectionState))?,
                    *value,
                )
                .map_err(|error| inspection_error(request, map_store_error(error))),
        }
    }
}

#[derive(Default)]
pub(super) struct TxState {
    pub(super) check_policy: RuleCheckPolicy,
    #[cfg(test)]
    pub(super) force_unpruned_inspection: bool,
    pub(super) entries: Vec<TxEntry>,
    pub(super) entry_payload_bytes: u128,
    pub(super) matched_var: Option<MatchValue>,
    pub(super) matched_var_name: Option<MatchValue>,
    pub(super) matched_vars: Vec<TxEntry<MatchValue, MatchValue>>,
    pub(super) matched_vars_payload_bytes: u128,
    pub(super) removed_units: Vec<usize>,
    pub(super) target_exclusions: Vec<ActiveTargetExclusion>,
    pub(super) request_body_processor: Option<super::super::super::BodyProcessorSelection>,
    pub(super) force_request_body_variable: Option<super::super::OnOff>,
    pub(super) retained_bytes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ActiveTargetExclusion {
    pub(super) unit: usize,
    pub(super) collection: CollectionKind,
    pub(super) selector: super::super::CompiledTargetSelector,
}

pub(super) struct CaptureEntry<V = MatchValue, K = MatchValue> {
    pub(super) key: K,
    pub(super) value: V,
}

#[derive(Clone, Copy)]
pub(super) enum CaptureKeySource<'a> {
    Numeric {
        index: usize,
        bytes: &'a [u8],
    },
    Named {
        program: usize,
        group: usize,
        bytes: &'a [u8],
    },
}
impl<'a> CaptureKeySource<'a> {
    pub(super) fn bytes(self) -> &'a [u8] {
        match self {
            Self::Numeric { bytes, .. } | Self::Named { bytes, .. } => bytes,
        }
    }
}
pub(super) trait StageCaptureKey: Sized {
    fn allocation_bytes(source: CaptureKeySource<'_>) -> usize;
    fn from_source(source: CaptureKeySource<'_>) -> Result<Self, RuntimeError>;
    fn heap_capacity(&self) -> usize;
}
impl StageCaptureKey for Vec<u8> {
    fn allocation_bytes(source: CaptureKeySource<'_>) -> usize {
        source.bytes().len()
    }
    fn from_source(source: CaptureKeySource<'_>) -> Result<Self, RuntimeError> {
        owned_copy(source.bytes())
    }
    fn heap_capacity(&self) -> usize {
        self.capacity()
    }
}

impl StageCaptureKey for MatchValue {
    fn allocation_bytes(source: CaptureKeySource<'_>) -> usize {
        source.bytes().len()
    }
    fn from_source(source: CaptureKeySource<'_>) -> Result<Self, RuntimeError> {
        owned_copy(source.bytes()).map(Self::Owned)
    }
    fn heap_capacity(&self) -> usize {
        self.owned_capacity()
    }
}

/// Resolved only against the generation bound to the collector's capture scratch.
pub(super) enum JsonCaptureKey {
    Numeric(usize),
    Named { program: usize, group: usize },
}
impl StageCaptureKey for JsonCaptureKey {
    fn allocation_bytes(_: CaptureKeySource<'_>) -> usize {
        0
    }
    fn from_source(source: CaptureKeySource<'_>) -> Result<Self, RuntimeError> {
        Ok(match source {
            CaptureKeySource::Numeric { index, .. } => Self::Numeric(index),
            CaptureKeySource::Named { program, group, .. } => Self::Named { program, group },
        })
    }
    fn heap_capacity(&self) -> usize {
        0
    }
}
impl JsonCaptureKey {
    pub(super) fn with_bytes<T, E: From<RuntimeError>>(
        &self,
        generation: &CompiledGeneration,
        operation: impl FnOnce(&[u8]) -> Result<T, E>,
    ) -> Result<T, E> {
        match *self {
            Self::Numeric(index) => {
                let (digits, start) = decimal_bytes(index);
                operation(&digits[start..])
            }
            Self::Named { program, group } => {
                let name = generation
                    .capture_regexes
                    .get(program)
                    .and_then(|regex| regex.capture_name(group))
                    .ok_or(RuntimeError::GenerationInvariant)?;
                operation(name.as_bytes())
            }
        }
    }
}

pub(super) struct CaptureBuilder<V = MatchValue, K = MatchValue> {
    pub(super) entries: Vec<CaptureEntry<V, K>>,
    pub(super) semantic_bytes: usize,
    pub(super) heap_bytes: usize,
}
impl<V, K> Default for CaptureBuilder<V, K> {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            semantic_bytes: 0,
            heap_bytes: 0,
        }
    }
}
impl<V, K> CaptureBuilder<V, K> {
    pub(super) fn clear(&mut self) {
        self.entries.clear();
        self.semantic_bytes = 0;
        self.heap_bytes = 0;
    }
    pub(super) fn retained_bytes(&self) -> Result<usize, RuntimeError> {
        self.entries
            .capacity()
            .checked_mul(size_of::<CaptureEntry<V, K>>())
            .and_then(|bytes| bytes.checked_add(self.heap_bytes))
            .ok_or(RuntimeError::Arithmetic)
    }
}

/// Capture sources never escape canonical extraction or borrow persisted storage.
#[derive(Clone, Copy)]
pub(super) enum CaptureSource<'a> {
    InputRange { input: &'a [u8], start: usize, len: usize },
    Fingerprint(crate::injection::Fingerprint),
}
impl CaptureSource<'_> {
    pub(super) fn bytes(&self) -> Result<&[u8], RuntimeError> {
        match self {
            Self::InputRange { input, start, len } => input
                .get(*start..start.checked_add(*len).ok_or(RuntimeError::Arithmetic)?)
                .ok_or(RuntimeError::GenerationInvariant),
            Self::Fingerprint(value) => Ok(value.as_bytes()),
        }
    }
}
pub(super) trait StageCapturePayload: Sized {
    fn allocation_bytes(source: CaptureSource<'_>) -> Result<usize, RuntimeError>;
    fn from_source(source: CaptureSource<'_>) -> Result<Self, RuntimeError>;
    fn heap_capacity(&self) -> usize;
}
impl StageCapturePayload for MatchValue {
    fn allocation_bytes(source: CaptureSource<'_>) -> Result<usize, RuntimeError> {
        Ok(source.bytes()?.len())
    }
    fn from_source(source: CaptureSource<'_>) -> Result<Self, RuntimeError> {
        owned_copy(source.bytes()?).map(Self::Owned)
    }
    fn heap_capacity(&self) -> usize {
        self.owned_capacity()
    }
}
/// Relative to the exact transformed stage subsequently copied into its outcome.
/// Generated SQLi fingerprints remain independent owned bytes, never input ranges.
pub(super) enum StageCaptureValue {
    InputRange { start: usize, len: usize },
    Fingerprint(crate::injection::Fingerprint),
}
impl StageCapturePayload for StageCaptureValue {
    fn allocation_bytes(source: CaptureSource<'_>) -> Result<usize, RuntimeError> {
        source.bytes()?;
        Ok(0)
    }
    fn from_source(source: CaptureSource<'_>) -> Result<Self, RuntimeError> {
        source.bytes()?;
        Ok(match source {
            CaptureSource::InputRange { start, len, .. } => Self::InputRange { start, len },
            CaptureSource::Fingerprint(value) => Self::Fingerprint(value),
        })
    }
    fn heap_capacity(&self) -> usize {
        0
    }
}

#[derive(Default)]
pub(super) struct MemberMatch {
    pub(super) captures: Vec<CaptureEntry>,
    pub(super) matched_var: Option<MatchValue>,
    pub(super) matched_var_name: Option<MatchValue>,
}

#[derive(Clone, Copy)]
pub(super) struct MatchStorage {
    pub(super) fixed_bytes: usize,
    pub(super) retained_limit: usize,
    pub(super) limit: usize,
}

impl MatchStorage {
    fn growth_target(self, capacity: usize) -> Result<usize, RuntimeError> {
        Ok(capacity
            .checked_mul(2)
            .ok_or(RuntimeError::Arithmetic)?
            .max(1)
            .min(self.limit))
    }

    /// Success-only admission for every possible query hit. Actual growth still
    /// validates allocator capacity in push; this never mutates runtime counters.
    pub(super) fn admits_growth(self, mut capacity: usize, matches: usize) -> bool {
        while capacity < matches {
            let Ok(target) = self.growth_target(capacity) else {
                return false;
            };
            let Some(peak) = capacity
                .checked_add(target)
                .and_then(|count| count.checked_mul(size_of::<MemberMatch>()))
                .and_then(|bytes| self.fixed_bytes.checked_add(bytes))
            else {
                return false;
            };
            if target <= capacity || peak > self.retained_limit {
                return false;
            }
            capacity = target;
        }
        true
    }

    pub(super) fn push(
        self,
        matches: &mut Vec<MemberMatch>,
        matched: MemberMatch,
        scratch_bytes: &mut usize,
        match_bytes: &mut usize,
    ) -> Result<(), RuntimeError> {
        if matches.len() == matches.capacity() {
            let target = self.growth_target(matches.capacity())?;
            let allocation = target
                .checked_mul(size_of::<MemberMatch>())
                .ok_or(RuntimeError::Arithmetic)?;
            let current = self
                .fixed_bytes
                .checked_add(*match_bytes)
                .ok_or(RuntimeError::Arithmetic)?;
            // Reallocation may temporarily retain the complete old allocation.
            if current.checked_add(allocation).ok_or(RuntimeError::Arithmetic)? > self.retained_limit {
                return Err(RuntimeError::RetainedByteLimit {
                    limit: self.retained_limit,
                });
            }
            matches
                .try_reserve_exact(target.checked_sub(matches.len()).ok_or(RuntimeError::Arithmetic)?)
                .map_err(|_| RuntimeError::Allocation)?;
            let actual = matches
                .capacity()
                .checked_mul(size_of::<MemberMatch>())
                .ok_or(RuntimeError::Arithmetic)?;
            if current.checked_add(actual).ok_or(RuntimeError::Arithmetic)? > self.retained_limit {
                // Drop an allocator overshoot instead of retaining unadmitted space.
                *matches = Vec::new();
                *scratch_bytes = scratch_bytes
                    .checked_sub(*match_bytes)
                    .ok_or(RuntimeError::Arithmetic)?;
                *match_bytes = 0;
                return Err(RuntimeError::RetainedByteLimit {
                    limit: self.retained_limit,
                });
            }
            *scratch_bytes = scratch_bytes
                .checked_sub(*match_bytes)
                .and_then(|bytes| bytes.checked_add(actual))
                .ok_or(RuntimeError::Arithmetic)?;
            *match_bytes = actual;
        }
        matches.push(matched);
        Ok(())
    }
}

pub(super) struct ChainBranch {
    pub(super) tx: TxState,
    pub(super) capture_keys: Vec<MatchValue>,
    pub(super) setvar_actions: usize,
}

/// One reversible edit staged by the current rule unit.
pub(super) enum TxUndo {
    /// One unit was inserted into `removed_units` at this index.
    InsertedRemovedUnit(usize),
    /// One exclusion was inserted into `target_exclusions` at this index.
    InsertedTargetExclusion(usize),
    /// The base value of the base entry at index `origin` (at unit start),
    /// saved on its first replacement. `None` once a removal took it over.
    SavedValue { origin: u32, value: Option<MatchValue> },
    /// The base entry at index `origin` was removed; holds its original key
    /// and value.
    RemovedEntry { origin: u32, entry: TxEntry },
}

/// Marks an entry the current unit appended (it has no base original).
const UNIT_ENTRY: u32 = u32::MAX;
/// Marks a base entry whose original has not been saved yet.
const NOT_SAVED: u32 = u32::MAX;
/// Steps scanned for a saved original before the mark vector is built.
const SCAN_LIMIT: usize = 16;
/// First capacity of the mark vector; it then grows geometrically.
const MIN_MARKS: usize = 64;

/// Per-entry staging state, parallel to `TxState::entries` once built.
#[derive(Clone, Copy)]
pub(super) struct EntryMark {
    /// Index of the entry in the base `entries` at unit start, or `UNIT_ENTRY`.
    pub(super) origin: u32,
    /// Index of its `TxUndo::SavedValue` step, or `NOT_SAVED`.
    pub(super) saved: u32,
}

/// Transaction-owned undo state for the rule unit being staged in place.
///
/// It saves the original of each base entry at most once per unit; later
/// edits of the same entry, and every value of an entry the unit itself
/// appended, are dropped exactly as a plain `set`/`delete` drops them. The
/// saved payload is therefore bounded by the base TX payload at unit start.
///
/// Surviving base entries keep their relative order and precede every entry
/// the unit appended, so while the unit has recorded fewer than `SCAN_LIMIT`
/// steps an entry's origin and saved original are derived by scanning the
/// steps (removed origins included). The per-entry mark vector is built only
/// when the unit records more steps. Both vectors are cleared, never reallocated, between units.
#[derive(Default)]
pub(super) struct TxJournal {
    pub(super) marks: Vec<EntryMark>,
    pub(super) marks_active: bool,
    /// Entry count at the unit's first entry edit; `None` before it.
    pub(super) base_len: Option<usize>,
    pub(super) steps: Vec<TxUndo>,
}

impl TxJournal {
    /// Base entries this unit removed so far (implicit layout only).
    fn removed_entries(&self) -> usize {
        self.steps
            .iter()
            .filter(|step| matches!(step, TxUndo::RemovedEntry { .. }))
            .count()
    }

    /// Base origin of the surviving base entry at `index` (implicit layout):
    /// the smallest origin with `index` surviving base entries before it.
    fn origin_of(&self, index: usize) -> u32 {
        let mut origin = index;
        loop {
            let removed_before = self
                .steps
                .iter()
                .filter(
                    |step| matches!(step, TxUndo::RemovedEntry { origin: removed, .. } if *removed as usize <= origin),
                )
                .count();
            let next = index + removed_before;
            if next == origin {
                return origin as u32;
            }
            origin = next;
        }
    }

    /// Current index of the surviving base entry `origin` (implicit layout).
    fn index_of(&self, origin: u32) -> usize {
        origin as usize
            - self
                .steps
                .iter()
                .filter(|step| matches!(step, TxUndo::RemovedEntry { origin: removed, .. } if *removed < origin))
                .count()
    }

    /// Base origin of `entries[index]` (`UNIT_ENTRY` when the unit appended
    /// it) and the step holding its saved original, if any.
    fn lookup(&self, base_len: usize, index: usize) -> (u32, Option<usize>) {
        if self.marks_active {
            return self.marks.get(index).map_or((UNIT_ENTRY, None), |mark| {
                (mark.origin, (mark.saved != NOT_SAVED).then_some(mark.saved as usize))
            });
        }
        // Surviving base entries precede every appended entry.
        if index >= base_len.saturating_sub(self.removed_entries()) {
            return (UNIT_ENTRY, None);
        }
        let origin = self.origin_of(index);
        let saved = self
            .steps
            .iter()
            .position(|step| matches!(step, TxUndo::SavedValue { origin: saved, .. } if *saved == origin));
        (origin, saved)
    }

    /// Build the marks from the implicit layout.
    fn activate_marks(&mut self, base_len: usize, entries: usize) -> Result<(), RuntimeError> {
        self.marks.clear();
        self.marks
            .try_reserve(entries.saturating_add(1).max(MIN_MARKS))
            .map_err(|_| RuntimeError::Allocation)?;
        let base_live = base_len.saturating_sub(self.removed_entries());
        for index in 0..entries {
            let origin = if index < base_live {
                self.origin_of(index)
            } else {
                UNIT_ENTRY
            };
            self.marks.push(EntryMark {
                origin,
                saved: NOT_SAVED,
            });
        }
        for step_index in 0..self.steps.len() {
            if let TxUndo::SavedValue { origin, value: Some(_) } = self.steps[step_index] {
                let index = self.index_of(origin);
                if let Some(mark) = self.marks.get_mut(index) {
                    mark.saved = step_index as u32;
                }
            }
        }
        self.marks_active = true;
        Ok(())
    }

    /// Reserve every slot an entry edit may need, before the edit, so
    /// recording can never fail after it.
    fn reserve_entry_edit(
        journal: &mut Option<&mut Self>,
        entries: usize,
        index: usize,
        edit: EntryEdit,
    ) -> Result<(), RuntimeError> {
        let Some(journal) = journal else {
            return Ok(());
        };
        let base_len = match journal.base_len {
            Some(base_len) => base_len,
            None => {
                // Origins and step indices stay below the sentinels: entry
                // counts are bounded by max_tx_variables and by memory long
                // before u32::MAX.
                if u32::try_from(entries)
                    .ok()
                    .filter(|count| *count != UNIT_ENTRY)
                    .is_none()
                {
                    return Err(RuntimeError::Arithmetic);
                }
                journal.base_len = Some(entries);
                entries
            }
        };
        // Keep inactive lookups bounded: they scan every step.
        if !journal.marks_active && journal.steps.len() >= SCAN_LIMIT {
            journal.activate_marks(base_len, entries)?;
        }
        match edit {
            EntryEdit::Replace => {
                let (origin, saved) = journal.lookup(base_len, index);
                if origin != UNIT_ENTRY && saved.is_none() {
                    journal.steps.try_reserve(1).map_err(|_| RuntimeError::Allocation)?;
                }
            }
            EntryEdit::Push => {
                if journal.marks_active {
                    journal.marks.try_reserve(1).map_err(|_| RuntimeError::Allocation)?;
                }
            }
            EntryEdit::Remove => {
                if journal.lookup(base_len, index).0 != UNIT_ENTRY {
                    journal.steps.try_reserve(1).map_err(|_| RuntimeError::Allocation)?;
                }
            }
        }
        Ok(())
    }

    /// `entries[index]` now holds a staged value; `displaced` is what it held.
    fn replaced(journal: Option<&mut Self>, index: usize, displaced: MatchValue) {
        let Some(journal) = journal else {
            return;
        };
        let Some(base_len) = journal.base_len else {
            return;
        };
        let (origin, saved) = journal.lookup(base_len, index);
        if origin == UNIT_ENTRY || saved.is_some() {
            return;
        }
        #[cfg(test)]
        measurements::record(Event::TxJournalPayloadBytes, displaced.len());
        let step = journal.steps.len() as u32;
        journal.steps.push(TxUndo::SavedValue {
            origin,
            value: Some(displaced),
        });
        if journal.marks_active {
            if let Some(mark) = journal.marks.get_mut(index) {
                mark.saved = step;
            }
        }
    }

    fn pushed(journal: Option<&mut Self>) {
        if let Some(journal) = journal.filter(|journal| journal.marks_active) {
            journal.marks.push(EntryMark {
                origin: UNIT_ENTRY,
                saved: NOT_SAVED,
            });
        }
    }

    /// `entries[index]` was removed; `entry` is what it held.
    fn removed(journal: Option<&mut Self>, index: usize, mut entry: TxEntry) {
        let Some(journal) = journal else {
            return;
        };
        let Some(base_len) = journal.base_len else {
            return;
        };
        let (origin, saved) = if journal.marks_active {
            if index >= journal.marks.len() {
                return;
            }
            let mark = journal.marks.remove(index);
            (mark.origin, (mark.saved != NOT_SAVED).then_some(mark.saved as usize))
        } else {
            journal.lookup(base_len, index)
        };
        if origin == UNIT_ENTRY {
            return;
        }
        // A value saved by an earlier replacement is the original; the staged
        // value is dropped.
        let saved = saved
            .and_then(|step| journal.steps.get_mut(step))
            .and_then(|step| match step {
                TxUndo::SavedValue { value, .. } => value.take(),
                _ => None,
            });
        #[cfg(test)]
        measurements::record(
            Event::TxJournalPayloadBytes,
            entry.key.len() + if saved.is_some() { 0 } else { entry.value.len() },
        );
        if let Some(original) = saved {
            entry.value = original;
        }
        journal.steps.push(TxUndo::RemovedEntry { origin, entry });
    }

    fn reserve(journal: &mut Option<&mut Self>) -> Result<(), RuntimeError> {
        match journal {
            Some(journal) => journal.steps.try_reserve(1).map_err(|_| RuntimeError::Allocation),
            None => Ok(()),
        }
    }

    fn record(journal: Option<&mut Self>, step: TxUndo) {
        if let Some(journal) = journal {
            journal.steps.push(step);
        }
    }

    /// Restore the base entries: original values back in place, entries the
    /// unit appended dropped, removed originals reinserted at their base
    /// indices in ascending order. Surviving base entries keep their relative
    /// order (edits never move them), and capacity never shrinks during a unit,
    /// so nothing allocates.
    fn rollback_entries(&mut self, entries: &mut Vec<TxEntry>) {
        let Some(base_len) = self.base_len else {
            return;
        };
        if self.marks_active {
            for (entry, mark) in entries.iter_mut().zip(&self.marks) {
                if let Some(TxUndo::SavedValue { value, .. }) = self.steps.get_mut(mark.saved as usize) {
                    if let Some(original) = value.take() {
                        entry.value = original;
                    }
                }
            }
            let marks = &self.marks;
            let mut position = 0_usize;
            entries.retain(|_| {
                let keep = marks.get(position).is_some_and(|mark| mark.origin != UNIT_ENTRY);
                position += 1;
                keep
            });
        } else {
            for step_index in 0..self.steps.len() {
                let TxUndo::SavedValue { origin, .. } = self.steps[step_index] else {
                    continue;
                };
                let index = self.index_of(origin);
                if let TxUndo::SavedValue { value, .. } = &mut self.steps[step_index] {
                    if let (Some(entry), Some(original)) = (entries.get_mut(index), value.take()) {
                        entry.value = original;
                    }
                }
            }
            entries.truncate(base_len.saturating_sub(self.removed_entries()));
        }
        self.steps.sort_unstable_by_key(|step| match step {
            TxUndo::RemovedEntry { origin, .. } => *origin,
            _ => UNIT_ENTRY,
        });
        for step in self.steps.drain(..) {
            if let TxUndo::RemovedEntry { origin, entry } = step {
                let origin = origin as usize;
                if origin <= entries.len() {
                    entries.insert(origin, entry);
                }
            }
        }
    }

    /// Commit: drop the saved originals and keep the capacity.
    pub(super) fn clear(&mut self) {
        self.marks.clear();
        self.marks_active = false;
        self.base_len = None;
        self.steps.clear();
    }
}

/// The kind of entry edit a journal slot is reserved for.
#[derive(Clone, Copy)]
enum EntryEdit {
    Replace,
    Push,
    Remove,
}

/// Whole fields a rolled-back unit restores, moved out by `TxState::begin_unit`.
pub(super) struct TxCheckpoint {
    matched_var: Option<MatchValue>,
    matched_var_name: Option<MatchValue>,
    matched_vars: Vec<TxEntry<MatchValue, MatchValue>>,
    matched_vars_payload_bytes: u128,
    entry_payload_bytes: u128,
    request_body_processor: Option<super::super::super::BodyProcessorSelection>,
    force_request_body_variable: Option<super::super::OnOff>,
    retained_bytes: usize,
}

#[derive(Clone)]
pub(super) enum GenerationHandle<'generation> {
    Borrowed(&'generation CompiledGeneration),
    Shared(Arc<CompiledGeneration>),
}

impl Deref for GenerationHandle<'_> {
    type Target = CompiledGeneration;

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Borrowed(generation) => generation,
            Self::Shared(generation) => generation,
        }
    }
}

impl TxState {
    pub(super) fn add_removed_unit(
        &mut self,
        unit: usize,
        limits: RuntimeLimits,
        mut journal: Option<&mut TxJournal>,
    ) -> Result<(), RuntimeError> {
        let Err(index) = self.removed_units.binary_search(&unit) else {
            return Ok(());
        };
        if self.removed_units.len().saturating_add(self.target_exclusions.len()) >= limits.max_ctl_entries {
            return Err(RuntimeError::CtlEntryLimit {
                limit: limits.max_ctl_entries,
            });
        }
        self.removed_units
            .try_reserve(1)
            .map_err(|_| RuntimeError::Allocation)?;
        TxJournal::reserve(&mut journal)?;
        self.removed_units.insert(index, unit);
        TxJournal::record(journal, TxUndo::InsertedRemovedUnit(index));
        self.refresh_retained_bytes();
        if self.retained_bytes > limits.max_tx_bytes {
            return Err(RuntimeError::TxByteLimit {
                limit: limits.max_tx_bytes,
            });
        }
        Ok(())
    }

    pub(super) fn add_target_exclusion(
        &mut self,
        entry: ActiveTargetExclusion,
        limits: RuntimeLimits,
        mut journal: Option<&mut TxJournal>,
    ) -> Result<(), RuntimeError> {
        let start = self
            .target_exclusions
            .partition_point(|existing| existing.unit < entry.unit);
        let end = start + self.target_exclusions[start..].partition_point(|existing| existing.unit <= entry.unit);
        if self.target_exclusions[start..end].contains(&entry) {
            return Ok(());
        }
        if self.removed_units.len().saturating_add(self.target_exclusions.len()) >= limits.max_ctl_entries {
            return Err(RuntimeError::CtlEntryLimit {
                limit: limits.max_ctl_entries,
            });
        }
        self.target_exclusions
            .try_reserve(1)
            .map_err(|_| RuntimeError::Allocation)?;
        TxJournal::reserve(&mut journal)?;
        self.target_exclusions.insert(end, entry);
        TxJournal::record(journal, TxUndo::InsertedTargetExclusion(end));
        self.refresh_retained_bytes();
        if self.retained_bytes > limits.max_tx_bytes {
            return Err(RuntimeError::TxByteLimit {
                limit: limits.max_tx_bytes,
            });
        }
        Ok(())
    }

    fn find_entry(
        &self,
        key: &[u8],
        request: &RequestState,
        mut json_budget: Option<&mut InspectionBudget>,
    ) -> Result<Option<usize>, RuntimeError> {
        for (index, entry) in self.entries.iter().enumerate() {
            if entry
                .key
                .bytes(request, json_budget.as_deref_mut())?
                .eq_ignore_ascii_case(key)
            {
                return Ok(Some(index));
            }
        }
        Ok(None)
    }

    pub(super) fn get(
        &self,
        key: &[u8],
        request: &RequestState,
        json_budget: Option<&mut InspectionBudget>,
    ) -> Result<Option<&MatchValue>, RuntimeError> {
        Ok(self
            .find_entry(key, request, json_budget)?
            .map(|index| &self.entries[index].value))
    }

    /// Public observation validates shared keys without consuming runtime work.
    pub(super) fn observed_get(&self, key: &[u8], request: &RequestState) -> Result<Option<&MatchValue>, RuntimeError> {
        for entry in &self.entries {
            if entry.key.observed_bytes(request)?.eq_ignore_ascii_case(key) {
                return Ok(Some(&entry.value));
            }
        }
        Ok(None)
    }

    pub(super) fn delete(
        &mut self,
        key: &[u8],
        request: &RequestState,
        json_budget: Option<&mut InspectionBudget>,
        mut journal: Option<&mut TxJournal>,
    ) -> Result<(), RuntimeError> {
        if let Some(index) = self.find_entry(key, request, json_budget)? {
            let entry = &self.entries[index];
            let removed = tx_payload_bytes(entry.key.len(), entry.value.len())?;
            let total = tx_payload_delta(self.entry_payload_bytes, removed, 0)?;
            TxJournal::reserve_entry_edit(&mut journal, self.entries.len(), index, EntryEdit::Remove)?;
            let entry = self.entries.remove(index);
            TxJournal::removed(journal, index, entry);
            self.entry_payload_bytes = total;
            self.refresh_retained_bytes();
        }
        Ok(())
    }

    pub(super) fn set(
        &mut self,
        key: impl Into<MatchValue>,
        value: impl Into<MatchValue>,
        limits: RuntimeLimits,
        request: &RequestState,
        mut json_budget: Option<&mut InspectionBudget>,
        mut journal: Option<&mut TxJournal>,
    ) -> Result<(), RuntimeError> {
        let key = key.into();
        let value = value.into();
        if let Some(index) = self.find_entry(key.bytes(request, json_budget.as_deref_mut())?, request, json_budget)? {
            let entry = &mut self.entries[index];
            let total = tx_payload_delta(self.entry_payload_bytes, entry.value.len() as u128, value.len() as u128)?;
            TxJournal::reserve_entry_edit(&mut journal, self.entries.len(), index, EntryEdit::Replace)?;
            let entry = &mut self.entries[index];
            let displaced = std::mem::replace(&mut entry.value, value);
            TxJournal::replaced(journal, index, displaced);
            self.entry_payload_bytes = total;
            self.refresh_retained_bytes();
        } else {
            if self.entries.len() == limits.max_tx_variables {
                return Err(RuntimeError::TxVariableLimit {
                    limit: limits.max_tx_variables,
                });
            }
            self.entries.try_reserve(1).map_err(|_| RuntimeError::Allocation)?;
            let payload = tx_payload_bytes(key.len(), value.len())?;
            let total = tx_payload_delta(self.entry_payload_bytes, 0, payload)?;
            TxJournal::reserve_entry_edit(&mut journal, self.entries.len(), self.entries.len(), EntryEdit::Push)?;
            self.entries.push(TxEntry { key, value });
            TxJournal::pushed(journal);
            self.entry_payload_bytes = total;
            self.refresh_retained_bytes();
        }
        if self.retained_bytes > limits.max_tx_bytes {
            return Err(RuntimeError::TxByteLimit {
                limit: limits.max_tx_bytes,
            });
        }
        Ok(())
    }

    /// Whole-state copy for a chain frontier branch. Rule units stage in place
    /// through `begin_unit` and a `TxJournal` instead of copying the map.
    pub(super) fn try_clone(&self) -> Result<Self, RuntimeError> {
        #[cfg(test)]
        measurements::record(Event::TxStateClones, 1);
        let entries = try_clone_entries(&self.entries)?;
        let mut matched_vars = Vec::new();
        matched_vars
            .try_reserve_exact(self.matched_vars.len())
            .map_err(|_| RuntimeError::Allocation)?;
        for entry in &self.matched_vars {
            matched_vars.push(TxEntry {
                key: entry.key.try_clone()?,
                value: entry.value.try_clone()?,
            });
        }
        let mut removed_units = Vec::new();
        removed_units
            .try_reserve_exact(self.removed_units.len())
            .map_err(|_| RuntimeError::Allocation)?;
        removed_units.extend_from_slice(&self.removed_units);
        let mut target_exclusions = Vec::new();
        target_exclusions
            .try_reserve_exact(self.target_exclusions.len())
            .map_err(|_| RuntimeError::Allocation)?;
        target_exclusions.extend_from_slice(&self.target_exclusions);
        let mut cloned = Self {
            check_policy: self.check_policy,
            #[cfg(test)]
            force_unpruned_inspection: self.force_unpruned_inspection,
            entries,
            entry_payload_bytes: self.entry_payload_bytes,
            matched_var: self.matched_var.as_ref().map(MatchValue::try_clone).transpose()?,
            matched_var_name: self.matched_var_name.as_ref().map(MatchValue::try_clone).transpose()?,
            matched_vars,
            matched_vars_payload_bytes: self.matched_vars_payload_bytes,
            removed_units,
            target_exclusions,
            request_body_processor: self.request_body_processor,
            force_request_body_variable: self.force_request_body_variable,
            retained_bytes: self.retained_bytes,
        };
        cloned.refresh_retained_bytes();
        Ok(cloned)
    }

    /// Start staging one matched rule unit in place. Prior-unit match observers
    /// are moved out (the unit starts without them) together with every scalar
    /// a later rollback restores wholesale; vector edits go to the journal.
    pub(super) fn begin_unit(&mut self) -> TxCheckpoint {
        let checkpoint = TxCheckpoint {
            matched_var: self.matched_var.take(),
            matched_var_name: self.matched_var_name.take(),
            matched_vars: std::mem::take(&mut self.matched_vars),
            matched_vars_payload_bytes: self.matched_vars_payload_bytes,
            entry_payload_bytes: self.entry_payload_bytes,
            request_body_processor: self.request_body_processor,
            force_request_body_variable: self.force_request_body_variable,
            retained_bytes: self.retained_bytes,
        };
        self.matched_vars_payload_bytes = 0;
        self.refresh_retained_bytes();
        checkpoint
    }

    /// Restore the saved entry originals, undo the ctl-list insertions in
    /// reverse order and restore the checkpoint, leaving the state exactly as
    /// it was before `begin_unit`. Originals move back; nothing is copied and
    /// no allocation is needed because vectors never shrink while a unit is
    /// staged.
    pub(super) fn rollback_unit(&mut self, journal: &mut TxJournal, checkpoint: TxCheckpoint) {
        // Ctl-list insertions undo in reverse order; entry steps wait.
        for step in journal.steps.iter().rev() {
            match *step {
                TxUndo::InsertedRemovedUnit(index) => {
                    if index < self.removed_units.len() {
                        self.removed_units.remove(index);
                    }
                }
                TxUndo::InsertedTargetExclusion(index) => {
                    if index < self.target_exclusions.len() {
                        self.target_exclusions.remove(index);
                    }
                }
                TxUndo::SavedValue { .. } | TxUndo::RemovedEntry { .. } => {}
            }
        }
        journal.rollback_entries(&mut self.entries);
        journal.clear();
        self.matched_var = checkpoint.matched_var;
        self.matched_var_name = checkpoint.matched_var_name;
        self.matched_vars = checkpoint.matched_vars;
        self.matched_vars_payload_bytes = checkpoint.matched_vars_payload_bytes;
        self.entry_payload_bytes = checkpoint.entry_payload_bytes;
        self.request_body_processor = checkpoint.request_body_processor;
        self.force_request_body_variable = checkpoint.force_request_body_variable;
        self.retained_bytes = checkpoint.retained_bytes;
    }

    /// Payload totals stay exact; only the public semantic charge saturates.
    pub(super) fn refresh_retained_bytes(&mut self) {
        self.retained_bytes = self
            .entries
            .len()
            .saturating_mul(size_of::<TxEntry>())
            .saturating_add(usize::try_from(self.entry_payload_bytes).unwrap_or(usize::MAX))
            .saturating_add(self.matched_var.as_ref().map_or(0, MatchValue::len))
            .saturating_add(self.matched_var_name.as_ref().map_or(0, MatchValue::len))
            .saturating_add(
                self.matched_vars
                    .len()
                    .saturating_mul(size_of::<TxEntry<MatchValue, MatchValue>>()),
            )
            .saturating_add(self.removed_units.len().saturating_mul(size_of::<usize>()))
            .saturating_add(
                self.target_exclusions
                    .len()
                    .saturating_mul(size_of::<ActiveTargetExclusion>()),
            )
            .saturating_add(usize::try_from(self.matched_vars_payload_bytes).unwrap_or(usize::MAX));
    }
}

pub(super) fn tx_payload_bytes(key: usize, value: usize) -> Result<u128, RuntimeError> {
    (key as u128).checked_add(value as u128).ok_or(RuntimeError::Arithmetic)
}

pub(super) fn tx_payload_delta(current: u128, removed: u128, added: u128) -> Result<u128, RuntimeError> {
    current
        .checked_sub(removed)
        .and_then(|bytes| bytes.checked_add(added))
        .ok_or(RuntimeError::Arithmetic)
}

fn try_clone_entries(entries: &[TxEntry]) -> Result<Vec<TxEntry>, RuntimeError> {
    let mut cloned = Vec::new();
    cloned
        .try_reserve_exact(entries.len())
        .map_err(|_| RuntimeError::Allocation)?;
    for entry in entries {
        cloned.push(TxEntry {
            key: entry.key.try_clone()?,
            value: entry.value.try_clone()?,
        });
    }
    Ok(cloned)
}

pub(super) fn setvar_value(
    generation: &CompiledGeneration,
    request: &RequestState,
    tx: &TxState,
    setvar: super::super::CompiledSetVar,
    member_limit: usize,
    json_budget: Option<&mut InspectionBudget>,
) -> Result<Vec<u8>, RuntimeError> {
    if let Some(constant) = setvar.constant {
        let (digits, start) = signed_decimal_bytes(constant);
        return owned_copy(&digits[start..]);
    }
    if let Some(template) = setvar.value {
        return expand_template::<true>(generation, request, tx, template, member_limit, json_budget);
    }
    Ok(Vec::new())
}

pub(super) fn owned_copy(value: &[u8]) -> Result<Vec<u8>, RuntimeError> {
    let mut output = Vec::new();
    output
        .try_reserve_exact(value.len())
        .map_err(|_| RuntimeError::Allocation)?;
    output.extend_from_slice(value);
    Ok(output)
}

pub(super) fn signed_decimal_bytes(value: i64) -> ([u8; 40], usize) {
    let negative = value < 0;
    let mut magnitude = value.unsigned_abs();
    let mut bytes = [0_u8; 40];
    let mut start = bytes.len();
    loop {
        start -= 1;
        bytes[start] = b'0' + (magnitude % 10) as u8;
        magnitude /= 10;
        if magnitude == 0 {
            break;
        }
    }
    if negative {
        start -= 1;
        bytes[start] = b'-';
    }
    (bytes, start)
}
