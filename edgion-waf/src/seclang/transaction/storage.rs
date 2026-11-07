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
    pub(super) fn add_removed_unit(&mut self, unit: usize, limits: RuntimeLimits) -> Result<(), RuntimeError> {
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
        self.removed_units.insert(index, unit);
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
        self.target_exclusions.insert(end, entry);
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
    ) -> Result<(), RuntimeError> {
        if let Some(index) = self.find_entry(key, request, json_budget)? {
            let entry = &self.entries[index];
            let removed = tx_payload_bytes(entry.key.len(), entry.value.len())?;
            let total = tx_payload_delta(self.entry_payload_bytes, removed, 0)?;
            self.entries.remove(index);
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
    ) -> Result<(), RuntimeError> {
        let key = key.into();
        let value = value.into();
        if let Some(index) = self.find_entry(key.bytes(request, json_budget.as_deref_mut())?, request, json_budget)? {
            let entry = &mut self.entries[index];
            let total = tx_payload_delta(self.entry_payload_bytes, entry.value.len() as u128, value.len() as u128)?;
            entry.value = value;
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
            self.entries.push(TxEntry { key, value });
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

    pub(super) fn try_clone(&self) -> Result<Self, RuntimeError> {
        self.try_clone_with_matches(true)
    }

    pub(super) fn try_clone_for_unit(&self) -> Result<Self, RuntimeError> {
        #[cfg(test)]
        measurements::record(Event::UnitTxClones, 1);
        self.try_clone_with_matches(false)
    }

    fn try_clone_with_matches(&self, keep_matches: bool) -> Result<Self, RuntimeError> {
        let entries = try_clone_entries(&self.entries)?;
        let matched_vars = if keep_matches {
            let mut cloned = Vec::new();
            cloned
                .try_reserve_exact(self.matched_vars.len())
                .map_err(|_| RuntimeError::Allocation)?;
            for entry in &self.matched_vars {
                cloned.push(TxEntry {
                    key: entry.key.try_clone()?,
                    value: entry.value.try_clone()?,
                });
            }
            cloned
        } else {
            Vec::new()
        };
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
            matched_var: if keep_matches {
                self.matched_var.as_ref().map(MatchValue::try_clone).transpose()?
            } else {
                None
            },
            matched_var_name: if keep_matches {
                self.matched_var_name.as_ref().map(MatchValue::try_clone).transpose()?
            } else {
                None
            },
            matched_vars,
            matched_vars_payload_bytes: if keep_matches {
                self.matched_vars_payload_bytes
            } else {
                0
            },
            removed_units,
            target_exclusions,
            request_body_processor: self.request_body_processor,
            force_request_body_variable: self.force_request_body_variable,
            retained_bytes: self.retained_bytes,
        };
        cloned.refresh_retained_bytes();
        Ok(cloned)
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
