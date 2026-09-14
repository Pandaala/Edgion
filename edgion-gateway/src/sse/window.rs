//! Canonical, policy-neutral, bounded hold-and-release window for SSE bytes.

use std::fmt;
use std::mem::size_of;

use bytes::{Bytes, BytesMut};

use super::chain::{ChainLimit, SliceChain, MAX_SLICE_HANDLES};
use super::{ByteRange, SseRecordRef};

pub const MAX_RAW_BYTES: usize = 1024 * 1024;
pub const MAX_RESIDENT_BYTES: usize = 1024 * 1024;
pub const MAX_RAW_RANGES: usize = 4_096;
pub const MAX_EVENTS: usize = 1_024;
pub const MAX_LINES: usize = 4_096;
pub const MAX_METADATA_BYTES: usize = 128 * 1024;
pub const MAX_BUCKETS: usize = 64;
pub const MAX_FRAGMENTS_PER_BUCKET: usize = 1_024;
pub const MAX_SNAPSHOT_BYTES: usize = 1024 * 1024;
pub const MAX_OVERLAP_BYTES: usize = 256 * 1024;
pub const MAX_REPLACEMENT_BYTES: usize = 1024 * 1024;
pub const MAX_COMBINED_RESIDENT_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawWindowPolicy {
    pub per_bucket_overlap_bytes: usize,
    pub global_overlap_bytes: usize,
    pub max_replacement_bytes: usize,
}

impl Default for RawWindowPolicy {
    fn default() -> Self {
        Self {
            per_bucket_overlap_bytes: MAX_OVERLAP_BYTES,
            global_overlap_bytes: MAX_OVERLAP_BYTES,
            max_replacement_bytes: MAX_REPLACEMENT_BYTES,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowStats {
    pub raw_bytes: usize,
    pub raw_ranges: usize,
    pub events: usize,
    pub lines: usize,
    pub current_semantic_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticWindow {
    pub key: LogicalBucketKey,
    pub context: String,
    pub current: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticReplacement {
    pub key: LogicalBucketKey,
    pub current: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogicalField {
    Content,
    ReasoningContent,
    ToolArguments { tool_call_index: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct LogicalBucketKey {
    pub choice_index: u32,
    pub field: LogicalField,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SemanticSource {
    Utf8Ranges(Vec<ByteRange>),
    /// One complete JSON string token, including its quotes.
    JsonStringTokenRanges(Vec<ByteRange>),
}

impl SemanticSource {
    fn ranges(&self) -> &[ByteRange] {
        match self {
            Self::Utf8Ranges(ranges) | Self::JsonStringTokenRanges(ranges) => ranges,
        }
    }

    fn ranges_mut(&mut self) -> &mut [ByteRange] {
        match self {
            Self::Utf8Ranges(ranges) | Self::JsonStringTokenRanges(ranges) => ranges,
        }
    }

    fn range_capacity(&self) -> usize {
        match self {
            Self::Utf8Ranges(ranges) | Self::JsonStringTokenRanges(ranges) => ranges.capacity(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticFragment {
    pub bucket: LogicalBucketKey,
    pub source: SemanticSource,
}

/// An owned decoder record. Counts are derived by the constructors and cannot
/// be supplied by adapters.
#[derive(Debug, Clone)]
pub struct WindowRecord {
    slices: Vec<Bytes>,
    line_count: usize,
    complete_event: bool,
    semantic_available: bool,
    fragments: Vec<SemanticFragment>,
}

impl WindowRecord {
    pub fn raw(slices: Vec<Bytes>) -> Self {
        let line_count = count_lines(&slices);
        Self {
            slices,
            line_count,
            complete_event: false,
            semantic_available: true,
            fragments: Vec::new(),
        }
    }

    /// Attach provider-located semantic ranges to an owned record. Range and
    /// metadata counts are computed from these values during `retain`.
    pub fn with_fragments(mut self, fragments: Vec<SemanticFragment>) -> Self {
        self.fragments = fragments;
        self
    }

    pub fn semantic_unavailable(mut self) -> Self {
        self.semantic_available = false;
        self
    }

    pub(crate) fn raw_len(&self) -> usize {
        self.slices.iter().map(Bytes::len).sum()
    }

    pub(crate) fn into_raw_slices(self) -> Vec<Bytes> {
        self.slices
    }

    pub(crate) fn raw_slices_cloned(&self) -> Vec<Bytes> {
        self.slices.clone()
    }

    pub(crate) fn metadata_bytes(&self) -> usize {
        let mut total = self
            .slices
            .capacity()
            .saturating_mul(size_of::<Bytes>())
            .saturating_add(self.fragments.capacity().saturating_mul(size_of::<SemanticFragment>()));
        for fragment in &self.fragments {
            total = total.saturating_add(fragment.source.range_capacity().saturating_mul(size_of::<ByteRange>()));
        }
        total
    }

    /// Canonical synchronous decoder-to-window adapter.
    pub fn from_sse_record(record: SseRecordRef<'_>) -> Self {
        match record {
            SseRecordRef::Preamble(bytes) | SseRecordRef::FramingContinuation(bytes) => {
                Self::raw(vec![Bytes::copy_from_slice(bytes)])
            }
            SseRecordRef::Block { event, .. } => Self {
                slices: event.raw().to_slices(),
                line_count: event.line_count(),
                complete_event: true,
                semantic_available: true,
                fragments: Vec::new(),
            },
            SseRecordRef::EosTail(raw) => Self::raw(raw.to_slices()),
            SseRecordRef::Opaque(raw) => Self::raw(raw.to_slices()).semantic_unavailable(),
        }
    }

    #[cfg(test)]
    fn complete(mut self) -> Self {
        self.complete_event = true;
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowLimit {
    RawBytes,
    ResidentBytes,
    SliceHandles,
    RawRanges,
    Events,
    Lines,
    MetadataBytes,
    Buckets,
    FragmentsPerBucket,
    SnapshotBytes,
    OverlapBytes,
    ReplacementBytes,
    CombinedResidentBytes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowError {
    Limit(WindowLimit),
    InvalidRange,
    SemanticUnavailable,
    InvalidUtf8,
    InvalidJsonString,
    InvalidPolicy,
    InvalidReplacement,
    NotCollecting,
    NotResolved,
    Finished,
}

impl fmt::Display for WindowError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SSE window error: {self:?}")
    }
}

impl std::error::Error for WindowError {}

#[derive(Debug)]
pub enum WindowDecision {
    Release,
    Replace(Bytes),
    Discard,
}

#[derive(Debug, PartialEq, Eq)]
pub enum WindowOutput {
    Release(Vec<Bytes>),
    Replace(Bytes),
    Discard,
}

#[derive(Debug, Clone, Copy)]
struct Limits {
    raw: usize,
    resident: usize,
    ranges: usize,
    events: usize,
    lines: usize,
    metadata: usize,
    buckets: usize,
    fragments: usize,
    snapshot: usize,
    #[allow(dead_code)]
    overlap: usize,
    replacement: usize,
    combined: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            raw: MAX_RAW_BYTES,
            resident: MAX_RESIDENT_BYTES,
            ranges: MAX_RAW_RANGES,
            events: MAX_EVENTS,
            lines: MAX_LINES,
            metadata: MAX_METADATA_BYTES,
            buckets: MAX_BUCKETS,
            fragments: MAX_FRAGMENTS_PER_BUCKET,
            snapshot: MAX_SNAPSHOT_BYTES,
            overlap: MAX_OVERLAP_BYTES,
            replacement: MAX_REPLACEMENT_BYTES,
            combined: MAX_COMBINED_RESIDENT_BYTES,
        }
    }
}

#[derive(Debug, Clone)]
struct StoredFragment {
    source: SemanticSource,
}

#[derive(Debug, Clone)]
struct Bucket {
    key: LogicalBucketKey,
    fragments: Vec<StoredFragment>,
    overlap: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Collecting,
    Resolved,
    Finished,
}

#[derive(Debug)]
pub struct RawWindow {
    limits: Limits,
    raw: SliceChain,
    raw_ranges: usize,
    events: usize,
    lines: usize,
    buckets: Vec<Bucket>,
    semantic_degraded: bool,
    semantic_failure: Option<WindowError>,
    state: State,
    policy: RawWindowPolicy,
}

impl Default for RawWindow {
    fn default() -> Self {
        Self::new()
    }
}

impl RawWindow {
    pub fn new() -> Self {
        Self::with_policy(RawWindowPolicy::default()).expect("default RawWindow policy is valid")
    }

    pub fn with_policy(policy: RawWindowPolicy) -> Result<Self, WindowError> {
        if policy.per_bucket_overlap_bytes > MAX_OVERLAP_BYTES
            || policy.global_overlap_bytes > MAX_OVERLAP_BYTES
            || policy.max_replacement_bytes > MAX_REPLACEMENT_BYTES
        {
            return Err(WindowError::InvalidPolicy);
        }
        Ok(Self::with_limits_and_policy(Limits::default(), policy))
    }

    #[allow(dead_code)]
    fn with_limits(limits: Limits) -> Self {
        let policy = RawWindowPolicy {
            per_bucket_overlap_bytes: limits.overlap,
            global_overlap_bytes: limits.overlap,
            max_replacement_bytes: limits.replacement,
        };
        Self::with_limits_and_policy(limits, policy)
    }

    fn with_limits_and_policy(limits: Limits, policy: RawWindowPolicy) -> Self {
        Self {
            limits,
            raw: SliceChain::default(),
            raw_ranges: 0,
            events: 0,
            lines: 0,
            buckets: Vec::new(),
            semantic_degraded: false,
            semantic_failure: None,
            state: State::Collecting,
            policy,
        }
    }

    pub fn stats(&mut self) -> Result<WindowStats, WindowError> {
        self.require_collecting()?;
        let current_semantic_bytes = self
            .materialize_current_values()
            .inspect_err(|error| {
                self.semantic_degraded = true;
                self.semantic_failure = Some(*error);
            })?
            .iter()
            .try_fold(0usize, |total, (_, value)| total.checked_add(value.len()))
            .ok_or(WindowError::Limit(WindowLimit::SnapshotBytes))?;
        Ok(WindowStats {
            raw_bytes: self.raw.len(),
            raw_ranges: self.raw_ranges,
            events: self.events,
            lines: self.lines,
            current_semantic_bytes,
        })
    }

    pub fn snapshot_all(&mut self) -> Result<Vec<SemanticWindow>, WindowError> {
        self.require_collecting()?;
        if self.semantic_degraded {
            return Err(WindowError::SemanticUnavailable);
        }
        let current = self.materialize_current_values().inspect_err(|error| {
            self.semantic_degraded = true;
            self.semantic_failure = Some(*error);
        })?;
        Ok(current
            .into_iter()
            .map(|(key, current)| {
                let context = self
                    .buckets
                    .binary_search_by_key(&key, |bucket| bucket.key)
                    .ok()
                    .map(|index| self.buckets[index].overlap.clone())
                    .unwrap_or_default();
                SemanticWindow { key, context, current }
            })
            .collect())
    }

    pub fn resolve_semantic_replace(
        &mut self,
        replacements: Vec<SemanticReplacement>,
    ) -> Result<WindowOutput, WindowError> {
        self.apply_semantic_replace(replacements, false)
    }

    pub fn preview_semantic_replace(&self, replacements: &[SemanticReplacement]) -> Result<usize, WindowError> {
        self.require_collecting()?;
        if let Some(error) = self.semantic_failure {
            return Err(error);
        }
        Ok(self.prepare_semantic_replace(replacements.to_vec())?.output.len())
    }

    pub fn finish_semantic_replace(
        &mut self,
        replacements: Vec<SemanticReplacement>,
    ) -> Result<WindowOutput, WindowError> {
        self.apply_semantic_replace(replacements, true)
    }

    fn apply_semantic_replace(
        &mut self,
        replacements: Vec<SemanticReplacement>,
        finish: bool,
    ) -> Result<WindowOutput, WindowError> {
        self.require_collecting()?;
        if let Some(error) = self.semantic_failure {
            return Err(error);
        }
        let prepared = self.prepare_semantic_replace(replacements)?;
        self.install_overlaps(prepared.overlaps);
        for bucket in &mut self.buckets {
            bucket.fragments.clear();
        }
        let _ = self.raw.take_raw_slices();
        self.clear_generation();
        if finish {
            self.buckets.clear();
            self.state = State::Finished;
        } else {
            self.state = State::Resolved;
        }
        Ok(WindowOutput::Replace(prepared.output))
    }

    /// Retain a record atomically. A failed append leaves the previous raw
    /// generation byte-identically releasable.
    pub fn retain(&mut self, record: WindowRecord) -> Result<(), WindowError> {
        self.retain_with_transient_metadata(record, 0)
    }

    fn retain_with_transient_metadata(
        &mut self,
        mut record: WindowRecord,
        transient_metadata: usize,
    ) -> Result<(), WindowError> {
        self.require_collecting()?;
        let record_len =
            checked_sum(record.slices.iter().map(Bytes::len)).ok_or(WindowError::Limit(WindowLimit::RawBytes))?;
        let base = self.raw.len();
        check_limit(
            base.checked_add(record_len)
                .ok_or(WindowError::Limit(WindowLimit::RawBytes))?,
            self.limits.raw,
            WindowLimit::RawBytes,
        )?;

        let semantic_ranges = validate_and_translate(&mut record.fragments, record_len, base)?;
        let next_ranges = self
            .raw_ranges
            .checked_add(1)
            .and_then(|value| value.checked_add(semantic_ranges))
            .ok_or(WindowError::Limit(WindowLimit::RawRanges))?;
        check_limit(next_ranges, self.limits.ranges, WindowLimit::RawRanges)?;
        let next_lines = self
            .lines
            .checked_add(record.line_count)
            .ok_or(WindowError::Limit(WindowLimit::Lines))?;
        check_limit(next_lines, self.limits.lines, WindowLimit::Lines)?;
        let next_events = self
            .events
            .checked_add(usize::from(record.complete_event))
            .ok_or(WindowError::Limit(WindowLimit::Events))?;
        check_limit(next_events, self.limits.events, WindowLimit::Events)?;
        let next_buckets = plan_buckets(&self.buckets, record.fragments, self.limits)?;

        let raw_plan = self
            .raw
            .plan_append(record.slices, self.limits.raw, self.limits.resident)
            .map_err(|limit| match limit {
                ChainLimit::Logical => WindowError::Limit(WindowLimit::RawBytes),
                ChainLimit::Resident => WindowError::Limit(WindowLimit::ResidentBytes),
            })?;
        check_limit(raw_plan.handle_count(), MAX_SLICE_HANDLES, WindowLimit::SliceHandles)?;
        check_limit(
            raw_plan.resident_capacity(),
            self.limits.resident,
            WindowLimit::ResidentBytes,
        )?;
        let metadata = metadata_charge(raw_plan.metadata_capacity(), &next_buckets)?;
        check_limit(
            metadata
                .checked_add(transient_metadata)
                .ok_or(WindowError::Limit(WindowLimit::MetadataBytes))?,
            self.limits.metadata,
            WindowLimit::MetadataBytes,
        )?;

        // The live generation, normalized incoming slices, prospective handle
        // vector/arena, and cloned bucket metadata coexist until commit.
        let current_metadata = metadata_charge(self.raw.slice_metadata_capacity(), &self.buckets)?;
        check_limit(
            Some(self.raw.resident_capacity())
                .and_then(|value| value.checked_add(overlap_capacity(&self.buckets).ok()?))
                .and_then(|value| value.checked_add(current_metadata))
                .and_then(|value| value.checked_add(raw_plan.scratch_resident()))
                .and_then(|value| value.checked_add(raw_plan.scratch_metadata()))
                .and_then(|value| value.checked_add(overlap_capacity(&next_buckets).ok()?))
                .and_then(|value| value.checked_add(metadata))
                .and_then(|value| value.checked_add(transient_metadata))
                .ok_or(WindowError::Limit(WindowLimit::CombinedResidentBytes))?,
            self.limits.combined,
            WindowLimit::CombinedResidentBytes,
        )?;

        self.raw.commit_append(raw_plan);
        self.raw_ranges = next_ranges;
        self.lines = next_lines;
        self.events = next_events;
        self.buckets = next_buckets;
        self.semantic_degraded |= !record.semantic_available;
        if !record.semantic_available {
            self.semantic_failure = Some(WindowError::SemanticUnavailable);
        }
        Ok(())
    }

    pub fn try_retain(&mut self, record: WindowRecord) -> Result<(), (WindowError, WindowRecord)> {
        // The original must remain owned by this function so an atomic failure
        // can return it byte-identically. Bytes payloads are ref-counted and
        // therefore not duplicated by the clone, but the original slice,
        // fragment, and range Vec allocations coexist with the prospective
        // RawWindow plan. Charge that complete metadata explicitly.
        let transient_metadata = record.metadata_bytes();
        match self.retain_with_transient_metadata(record.clone(), transient_metadata) {
            Ok(()) => Ok(()),
            Err(error) => Err((error, record)),
        }
    }

    pub fn snapshot(&mut self, bucket: LogicalBucketKey) -> Result<WindowView<'_>, WindowError> {
        self.require_collecting()?;
        if self.semantic_degraded {
            return Err(WindowError::SemanticUnavailable);
        }
        if self.buckets.binary_search_by_key(&bucket, |entry| entry.key).is_err() {
            return Err(WindowError::SemanticUnavailable);
        }
        Ok(WindowView { window: self, bucket })
    }

    /// Resolve exactly once. Semantic failure degrades this generation to a
    /// raw-only release instead of trapping its already-retained bytes.
    pub fn resolve(&mut self, decision: WindowDecision) -> Result<WindowOutput, WindowError> {
        self.require_collecting()?;
        if matches!(&decision, WindowDecision::Discard) {
            let output = self.apply(PreparedDecision::Discard);
            self.clear_generation();
            for bucket in &mut self.buckets {
                bucket.fragments.clear();
                bucket.overlap.clear();
            }
            self.state = State::Resolved;
            return Ok(output);
        }
        if let Some(error) = self.semantic_failure {
            if !matches!(&decision, WindowDecision::Release) {
                return Err(error);
            }
        }
        let prepared = self.prepare_decision(decision)?;
        if !self.semantic_degraded {
            let overlaps = match self.prepare_overlaps() {
                Ok(overlaps) => overlaps,
                Err(error) => {
                    self.semantic_degraded = true;
                    self.semantic_failure = Some(error);
                    return Err(error);
                }
            };
            self.install_overlaps(overlaps);
        }
        for bucket in &mut self.buckets {
            bucket.fragments.clear();
        }
        let output = self.apply(prepared);
        self.clear_generation();
        self.state = State::Resolved;
        Ok(output)
    }

    pub fn start_next(&mut self) -> Result<(), WindowError> {
        match self.state {
            State::Resolved => {
                self.state = State::Collecting;
                Ok(())
            }
            State::Collecting => Err(WindowError::NotResolved),
            State::Finished => Err(WindowError::Finished),
        }
    }

    pub fn finish(&mut self, decision: WindowDecision) -> Result<WindowOutput, WindowError> {
        self.require_collecting()?;
        if matches!(&decision, WindowDecision::Discard) {
            let output = self.apply(PreparedDecision::Discard);
            self.clear_generation();
            self.buckets.clear();
            self.state = State::Finished;
            return Ok(output);
        }
        if let Some(error) = self.semantic_failure {
            if !matches!(&decision, WindowDecision::Release) {
                return Err(error);
            }
        }
        let prepared = self.prepare_decision(decision)?;
        let output = self.apply(prepared);
        self.clear_generation();
        self.buckets.clear();
        self.state = State::Finished;
        Ok(output)
    }

    fn prepare_decision(&self, decision: WindowDecision) -> Result<PreparedDecision, WindowError> {
        match decision {
            WindowDecision::Release => {
                let output = Vec::with_capacity(self.raw.handle_count());
                let output_metadata = output
                    .capacity()
                    .checked_mul(size_of::<Bytes>())
                    .ok_or(WindowError::Limit(WindowLimit::MetadataBytes))?;
                check_limit(output_metadata, self.limits.metadata, WindowLimit::MetadataBytes)?;
                check_limit(
                    self.raw
                        .resident_capacity()
                        .checked_add(overlap_capacity(&self.buckets)?)
                        .and_then(|value| value.checked_add(output_metadata))
                        .ok_or(WindowError::Limit(WindowLimit::CombinedResidentBytes))?,
                    self.limits.combined,
                    WindowLimit::CombinedResidentBytes,
                )?;
                Ok(PreparedDecision::Release(output))
            }
            WindowDecision::Discard => Ok(PreparedDecision::Discard),
            WindowDecision::Replace(bytes) => {
                check_limit(bytes.len(), self.limits.replacement, WindowLimit::ReplacementBytes)?;
                let (replacement, replacement_resident) = normalize_replacement(bytes, self.limits.replacement)?;
                let replacement_metadata = size_of::<Bytes>();
                check_limit(
                    self.raw
                        .resident_capacity()
                        .checked_add(overlap_capacity(&self.buckets)?)
                        .and_then(|value| value.checked_add(replacement_resident))
                        .and_then(|value| value.checked_add(replacement_metadata))
                        .ok_or(WindowError::Limit(WindowLimit::CombinedResidentBytes))?,
                    self.limits.combined,
                    WindowLimit::CombinedResidentBytes,
                )?;
                Ok(PreparedDecision::Replace(replacement))
            }
        }
    }

    fn prepare_overlaps(&mut self) -> Result<Vec<(LogicalBucketKey, String)>, WindowError> {
        let mut values = Vec::with_capacity(self.buckets.len());
        let mut scratch = values
            .capacity()
            .saturating_mul(size_of::<(LogicalBucketKey, String)>());
        for index in 0..self.buckets.len() {
            let value = self.materialize_bucket(index)?;
            scratch = scratch
                .checked_add(value.capacity())
                .ok_or(WindowError::Limit(WindowLimit::SnapshotBytes))?;
            check_limit(scratch, self.limits.snapshot, WindowLimit::SnapshotBytes)?;
            values.push((self.buckets[index].key, value));
        }
        check_limit(
            self.raw
                .resident_capacity()
                .checked_add(overlap_capacity(&self.buckets)?)
                .and_then(|value| value.checked_add(scratch))
                .ok_or(WindowError::Limit(WindowLimit::CombinedResidentBytes))?,
            self.limits.combined,
            WindowLimit::CombinedResidentBytes,
        )?;
        self.prepare_overlaps_from_values(values)
    }

    fn prepare_overlaps_from_values(
        &self,
        values: Vec<(LogicalBucketKey, String)>,
    ) -> Result<Vec<(LogicalBucketKey, String)>, WindowError> {
        let needs: Vec<_> = values
            .iter()
            .map(|(_, value)| value.len().min(self.policy.per_bucket_overlap_bytes))
            .collect();
        let allocations = max_min_allocations(&needs, self.policy.global_overlap_bytes);
        let mut result = Vec::with_capacity(values.len());
        let result_metadata = result
            .capacity()
            .saturating_mul(size_of::<(LogicalBucketKey, String)>());
        let mut overlap_resident = 0usize;
        for ((key, value), allocation) in values.into_iter().zip(allocations) {
            let mut start = value.len().saturating_sub(allocation);
            while start < value.len() && !value.is_char_boundary(start) {
                start += 1;
            }
            let overlap = value[start..].to_owned();
            overlap_resident = overlap_resident
                .checked_add(overlap.capacity())
                .ok_or(WindowError::Limit(WindowLimit::OverlapBytes))?;
            check_limit(
                overlap_resident,
                self.policy.global_overlap_bytes,
                WindowLimit::OverlapBytes,
            )?;
            result.push((key, overlap));
        }
        check_limit(
            self.raw
                .resident_capacity()
                .checked_add(overlap_capacity(&self.buckets)?)
                .and_then(|value| value.checked_add(overlap_resident))
                .and_then(|value| value.checked_add(result_metadata))
                .ok_or(WindowError::Limit(WindowLimit::CombinedResidentBytes))?,
            self.limits.combined,
            WindowLimit::CombinedResidentBytes,
        )?;
        Ok(result)
    }

    fn materialize_current_values(&self) -> Result<Vec<(LogicalBucketKey, String)>, WindowError> {
        let mut values = Vec::with_capacity(self.buckets.len());
        let mut total = values
            .capacity()
            .saturating_mul(size_of::<(LogicalBucketKey, String)>());
        for (index, bucket) in self.buckets.iter().enumerate() {
            let value = self.materialize_bucket_current(index)?;
            total = total
                .checked_add(value.capacity())
                .ok_or(WindowError::Limit(WindowLimit::SnapshotBytes))?;
            check_limit(total, self.limits.snapshot, WindowLimit::SnapshotBytes)?;
            values.push((bucket.key, value));
        }
        Ok(values)
    }

    fn materialize_bucket_current(&self, index: usize) -> Result<String, WindowError> {
        self.materialize_bucket_parts(index, false)
    }

    fn install_overlaps(&mut self, overlaps: Vec<(LogicalBucketKey, String)>) {
        for bucket in &mut self.buckets {
            bucket.overlap.clear();
        }
        for (key, overlap) in overlaps {
            let index = self
                .buckets
                .binary_search_by_key(&key, |bucket| bucket.key)
                .expect("prepared overlap keys must come from the current bucket set");
            self.buckets[index].overlap = overlap;
        }
    }

    fn materialize_bucket(&self, index: usize) -> Result<String, WindowError> {
        self.materialize_bucket_parts(index, true)
    }

    fn materialize_bucket_parts(&self, index: usize, include_overlap: bool) -> Result<String, WindowError> {
        let bucket = &self.buckets[index];
        let prefix_len = if include_overlap { bucket.overlap.len() } else { 0 };
        let encoded_len = bucket.fragments.iter().try_fold(prefix_len, |total, fragment| {
            fragment.source.ranges().iter().try_fold(total, |total, range| {
                total
                    .checked_add(range.len())
                    .ok_or(WindowError::Limit(WindowLimit::SnapshotBytes))
            })
        })?;
        check_limit(encoded_len, self.limits.snapshot, WindowLimit::SnapshotBytes)?;
        let mut out = String::with_capacity(encoded_len);
        check_limit(out.capacity(), self.limits.snapshot, WindowLimit::SnapshotBytes)?;
        if include_overlap {
            out.push_str(&bucket.overlap);
        }
        for fragment in &bucket.fragments {
            let length = checked_sum(fragment.source.ranges().iter().map(|range| range.len()))
                .ok_or(WindowError::Limit(WindowLimit::SnapshotBytes))?;
            let mut scratch = Vec::with_capacity(length);
            check_limit(scratch.capacity(), self.limits.snapshot, WindowLimit::SnapshotBytes)?;
            for &range in fragment.source.ranges() {
                self.raw
                    .for_each_range(range, |part| scratch.extend_from_slice(part))
                    .ok_or(WindowError::InvalidRange)?;
            }
            match &fragment.source {
                SemanticSource::Utf8Ranges(_) => {
                    out.push_str(std::str::from_utf8(&scratch).map_err(|_| WindowError::InvalidUtf8)?)
                }
                SemanticSource::JsonStringTokenRanges(_) => {
                    let decoded: String =
                        serde_json::from_slice(&scratch).map_err(|_| WindowError::InvalidJsonString)?;
                    out.push_str(&decoded);
                }
            }
            check_limit(out.len(), self.limits.snapshot, WindowLimit::SnapshotBytes)?;
            check_limit(out.capacity(), self.limits.snapshot, WindowLimit::SnapshotBytes)?;
            check_limit(
                self.raw
                    .resident_capacity()
                    .checked_add(overlap_capacity(&self.buckets)?)
                    .and_then(|value| value.checked_add(out.capacity()))
                    .and_then(|value| value.checked_add(scratch.capacity()))
                    .ok_or(WindowError::Limit(WindowLimit::CombinedResidentBytes))?,
                self.limits.combined,
                WindowLimit::CombinedResidentBytes,
            )?;
        }
        Ok(out)
    }

    fn prepare_semantic_replace(
        &self,
        mut replacements: Vec<SemanticReplacement>,
    ) -> Result<PreparedSemanticReplace, WindowError> {
        replacements.sort_by_key(|entry| entry.key);
        if replacements.len() != self.buckets.len()
            || replacements.windows(2).any(|pair| pair[0].key == pair[1].key)
            || replacements
                .iter()
                .zip(&self.buckets)
                .any(|(replacement, bucket)| replacement.key != bucket.key)
        {
            return Err(WindowError::InvalidReplacement);
        }
        let replacement_text_bytes = replacements
            .iter()
            .try_fold(0usize, |total, replacement| {
                total.checked_add(replacement.current.len())
            })
            .ok_or(WindowError::Limit(WindowLimit::ReplacementBytes))?;
        check_limit(
            replacement_text_bytes,
            self.policy.max_replacement_bytes,
            WindowLimit::ReplacementBytes,
        )?;

        let mut edits = Vec::new();
        for (bucket, replacement) in self.buckets.iter().zip(&replacements) {
            for (index, fragment) in bucket.fragments.iter().enumerate() {
                let SemanticSource::JsonStringTokenRanges(ranges) = &fragment.source else {
                    return Err(WindowError::InvalidReplacement);
                };
                if ranges.len() != 1 {
                    return Err(WindowError::InvalidReplacement);
                }
                let encoded = if index == 0 {
                    serde_json::to_vec(&replacement.current).map_err(|_| WindowError::InvalidReplacement)?
                } else {
                    b"\"\"".to_vec()
                };
                edits.push((ranges[0], encoded));
            }
        }
        edits.sort_by_key(|(range, _)| range.start);
        if edits.windows(2).any(|pair| pair[0].0.end > pair[1].0.start) {
            return Err(WindowError::InvalidRange);
        }
        let output_len = edits
            .iter()
            .try_fold(self.raw.len(), |len, (range, encoded)| {
                len.checked_sub(range.len())?.checked_add(encoded.len())
            })
            .ok_or(WindowError::Limit(WindowLimit::ReplacementBytes))?;
        check_limit(output_len, self.limits.replacement, WindowLimit::ReplacementBytes)?;
        let mut output = BytesMut::with_capacity(output_len);
        check_limit(
            output.capacity(),
            self.limits.replacement,
            WindowLimit::ReplacementBytes,
        )?;
        let mut cursor = 0;
        for (range, encoded) in edits {
            self.raw
                .for_each_range(ByteRange::new(cursor, range.start), |part| {
                    output.extend_from_slice(part)
                })
                .ok_or(WindowError::InvalidRange)?;
            output.extend_from_slice(&encoded);
            cursor = range.end;
        }
        self.raw
            .for_each_range(ByteRange::new(cursor, self.raw.len()), |part| {
                output.extend_from_slice(part)
            })
            .ok_or(WindowError::InvalidRange)?;
        let overlaps = self.prepare_overlaps_from_values(
            replacements
                .into_iter()
                .map(|entry| (entry.key, entry.current))
                .collect(),
        )?;
        let overlap_resident = checked_sum(overlaps.iter().map(|(_, value)| value.capacity()))
            .ok_or(WindowError::Limit(WindowLimit::OverlapBytes))?;
        check_limit(
            self.raw
                .resident_capacity()
                .checked_add(output.capacity())
                .and_then(|value| value.checked_add(overlap_capacity(&self.buckets).ok()?))
                .and_then(|value| value.checked_add(overlap_resident))
                .ok_or(WindowError::Limit(WindowLimit::CombinedResidentBytes))?,
            self.limits.combined,
            WindowLimit::CombinedResidentBytes,
        )?;
        Ok(PreparedSemanticReplace {
            output: output.freeze(),
            overlaps,
        })
    }

    fn apply(&mut self, decision: PreparedDecision) -> WindowOutput {
        match decision {
            PreparedDecision::Release(output) => WindowOutput::Release(self.raw.take_raw_slices_into(output)),
            PreparedDecision::Replace(bytes) => {
                let _ = self.raw.take_raw_slices();
                WindowOutput::Replace(bytes)
            }
            PreparedDecision::Discard => {
                let _ = self.raw.take_raw_slices();
                WindowOutput::Discard
            }
        }
    }

    fn clear_generation(&mut self) {
        self.raw_ranges = 0;
        self.events = 0;
        self.lines = 0;
        self.semantic_degraded = false;
        self.semantic_failure = None;
    }

    fn require_collecting(&self) -> Result<(), WindowError> {
        match self.state {
            State::Collecting => Ok(()),
            State::Resolved => Err(WindowError::NotCollecting),
            State::Finished => Err(WindowError::Finished),
        }
    }

    #[cfg(test)]
    fn accounting(&self) -> (usize, usize, usize, usize, usize) {
        (
            self.raw.len(),
            self.raw.resident_capacity(),
            self.raw.handle_count(),
            self.raw.coalesce_count(),
            self.raw.copied_bytes(),
        )
    }
}

pub struct WindowView<'a> {
    window: &'a mut RawWindow,
    bucket: LogicalBucketKey,
}

impl WindowView<'_> {
    pub fn materialize(self) -> Result<String, WindowError> {
        let index = self
            .window
            .buckets
            .binary_search_by_key(&self.bucket, |entry| entry.key)
            .map_err(|_| WindowError::SemanticUnavailable)?;
        match self.window.materialize_bucket(index) {
            Ok(value) => Ok(value),
            Err(error) => {
                self.window.semantic_degraded = true;
                self.window.semantic_failure = Some(error);
                Err(error)
            }
        }
    }
}

enum PreparedDecision {
    Release(Vec<Bytes>),
    Replace(Bytes),
    Discard,
}

struct PreparedSemanticReplace {
    output: Bytes,
    overlaps: Vec<(LogicalBucketKey, String)>,
}

fn validate_and_translate(
    fragments: &mut [SemanticFragment],
    record_len: usize,
    base: usize,
) -> Result<usize, WindowError> {
    let mut count = 0usize;
    for fragment in fragments {
        let ranges = fragment.source.ranges_mut();
        if ranges.is_empty() {
            return Err(WindowError::InvalidRange);
        }
        let mut previous_end = 0;
        for range in ranges {
            if range.start > range.end || range.end > record_len || range.start < previous_end {
                return Err(WindowError::InvalidRange);
            }
            previous_end = range.end;
            range.start = range.start.checked_add(base).ok_or(WindowError::InvalidRange)?;
            range.end = range.end.checked_add(base).ok_or(WindowError::InvalidRange)?;
            count = count.checked_add(1).ok_or(WindowError::Limit(WindowLimit::RawRanges))?;
        }
    }
    Ok(count)
}

fn plan_buckets(
    current: &[Bucket],
    fragments: Vec<SemanticFragment>,
    limits: Limits,
) -> Result<Vec<Bucket>, WindowError> {
    let mut next = current.to_vec();
    for fragment in fragments {
        match next.binary_search_by_key(&fragment.bucket, |bucket| bucket.key) {
            Ok(index) => {
                check_limit(
                    next[index].fragments.len() + 1,
                    limits.fragments,
                    WindowLimit::FragmentsPerBucket,
                )?;
                next[index].fragments.push(StoredFragment {
                    source: fragment.source,
                });
            }
            Err(index) => {
                check_limit(next.len() + 1, limits.buckets, WindowLimit::Buckets)?;
                next.insert(
                    index,
                    Bucket {
                        key: fragment.bucket,
                        fragments: vec![StoredFragment {
                            source: fragment.source,
                        }],
                        overlap: String::new(),
                    },
                );
            }
        }
    }
    Ok(next)
}

fn metadata_charge(slice_metadata: usize, buckets: &Vec<Bucket>) -> Result<usize, WindowError> {
    let mut total = slice_metadata
        .checked_add(buckets.capacity().saturating_mul(size_of::<Bucket>()))
        .ok_or(WindowError::Limit(WindowLimit::MetadataBytes))?;
    for bucket in buckets {
        total = total
            .checked_add(bucket.fragments.capacity().saturating_mul(size_of::<StoredFragment>()))
            .ok_or(WindowError::Limit(WindowLimit::MetadataBytes))?;
        for fragment in &bucket.fragments {
            total = total
                .checked_add(fragment.source.range_capacity().saturating_mul(size_of::<ByteRange>()))
                .ok_or(WindowError::Limit(WindowLimit::MetadataBytes))?;
        }
    }
    Ok(total)
}

fn max_min_allocations(needs: &[usize], budget: usize) -> Vec<usize> {
    let mut allocations = vec![0; needs.len()];
    let mut active: Vec<_> = (0..needs.len()).collect();
    let mut remaining = budget;
    while !active.is_empty() && remaining != 0 {
        let share = remaining / active.len();
        let remainder = remaining % active.len();
        let mut used = 0;
        let mut next = Vec::new();
        for (position, &index) in active.iter().enumerate() {
            let offered = share + usize::from(position < remainder);
            let granted = offered.min(needs[index].saturating_sub(allocations[index]));
            allocations[index] += granted;
            used += granted;
            if allocations[index] < needs[index] {
                next.push(index);
            }
        }
        if used == 0 {
            break;
        }
        remaining -= used;
        active = next;
    }
    allocations
}

fn normalize_replacement(bytes: Bytes, limit: usize) -> Result<(Bytes, usize), WindowError> {
    match bytes.try_into_mut() {
        Ok(bytes) if bytes.capacity() == bytes.len() => {
            let resident = bytes.capacity();
            Ok((bytes.freeze(), resident))
        }
        Ok(bytes) => copy_exact(&bytes, limit),
        Err(bytes) => copy_exact(&bytes, limit),
    }
}

fn copy_exact(bytes: &[u8], limit: usize) -> Result<(Bytes, usize), WindowError> {
    let mut copy = BytesMut::with_capacity(bytes.len());
    check_limit(copy.capacity(), limit, WindowLimit::ReplacementBytes)?;
    copy.extend_from_slice(bytes);
    let resident = copy.capacity();
    Ok((copy.freeze(), resident))
}

fn count_lines(slices: &[Bytes]) -> usize {
    let mut lines = 0usize;
    let mut skip_lf = false;
    for byte in slices.iter().flat_map(|bytes| bytes.iter().copied()) {
        if skip_lf {
            skip_lf = false;
            if byte == b'\n' {
                continue;
            }
        }
        match byte {
            b'\r' => {
                lines = lines.saturating_add(1);
                skip_lf = true;
            }
            b'\n' => lines = lines.saturating_add(1),
            _ => {}
        }
    }
    lines
}

fn overlap_capacity(buckets: &[Bucket]) -> Result<usize, WindowError> {
    checked_sum(buckets.iter().map(|bucket| bucket.overlap.capacity()))
        .ok_or(WindowError::Limit(WindowLimit::OverlapBytes))
}

fn checked_sum(values: impl IntoIterator<Item = usize>) -> Option<usize> {
    values.into_iter().try_fold(0usize, usize::checked_add)
}

fn check_limit(value: usize, limit: usize, kind: WindowLimit) -> Result<(), WindowError> {
    if value > limit {
        Err(WindowError::Limit(kind))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::field_reassign_with_default)]

    use super::super::SseDecoder;
    use super::*;

    fn key(choice: u32, field: LogicalField) -> LogicalBucketKey {
        LogicalBucketKey {
            choice_index: choice,
            field,
        }
    }

    fn fragment(bucket: LogicalBucketKey, ranges: Vec<ByteRange>) -> SemanticFragment {
        SemanticFragment {
            bucket,
            source: SemanticSource::Utf8Ranges(ranges),
        }
    }

    fn event(raw: Bytes, bucket: LogicalBucketKey, ranges: Vec<ByteRange>) -> WindowRecord {
        WindowRecord::raw(vec![raw])
            .complete()
            .with_fragments(vec![fragment(bucket, ranges)])
    }

    fn released(output: WindowOutput) -> Vec<u8> {
        match output {
            WindowOutput::Release(chunks) => chunks.into_iter().flatten().collect(),
            output => panic!("expected release, got {output:?}"),
        }
    }

    #[test]
    fn decoder_adapter_releases_every_record_byte_identically() {
        let source = Bytes::from_static(b"\xef\xbb\xbf: c\r\ndata: one\r\n\r\ndata: two\n\n");
        for split in 0..=source.len() {
            let mut decoder = SseDecoder::with_default_limit();
            let mut window = RawWindow::new();
            decoder.feed_records(source.slice(..split), |record| {
                window.retain(WindowRecord::from_sse_record(record)).unwrap()
            });
            decoder.feed_records(source.slice(split..), |record| {
                window.retain(WindowRecord::from_sse_record(record)).unwrap()
            });
            decoder.finish_records(|record| window.retain(WindowRecord::from_sse_record(record)).unwrap());
            assert_eq!(released(window.finish(WindowDecision::Release).unwrap()), source);
        }
    }

    #[test]
    fn raw_identity_and_cross_slice_semantics() {
        let raw = Bytes::from_static("a€𐍈".as_bytes());
        for split in 0..=raw.len() {
            let bucket = key(0, LogicalField::Content);
            let mut window = RawWindow::new();
            window
                .retain(
                    WindowRecord::raw(vec![raw.slice(..split), raw.slice(split..)])
                        .with_fragments(vec![fragment(bucket, vec![ByteRange::new(0, raw.len())])]),
                )
                .unwrap();
            assert_eq!(window.snapshot(bucket).unwrap().materialize().unwrap(), "a€𐍈");
            assert_eq!(released(window.resolve(WindowDecision::Release).unwrap()), raw);
        }
    }

    #[test]
    fn json_escape_semantics_split_at_every_byte() {
        let raw = Bytes::from_static(br#""h\u00e9\ud83d\ude00\\\"""#);
        for split in 0..=raw.len() {
            let bucket = key(0, LogicalField::Content);
            let mut window = RawWindow::new();
            let source = SemanticSource::JsonStringTokenRanges(vec![ByteRange::new(0, raw.len())]);
            window
                .retain(
                    WindowRecord::raw(vec![raw.slice(..split), raw.slice(split..)])
                        .with_fragments(vec![SemanticFragment { bucket, source }]),
                )
                .unwrap();
            assert_eq!(window.snapshot(bucket).unwrap().materialize().unwrap(), "hé😀\\\"");
        }
    }

    #[test]
    fn semantic_failure_permits_atomic_raw_only_release_and_next_generation() {
        for invalid in [Bytes::from_static(b"\xff"), Bytes::from_static(b"not-json")] {
            let bucket = key(0, LogicalField::Content);
            let source = if invalid[0] == 0xff {
                SemanticSource::Utf8Ranges(vec![ByteRange::new(0, invalid.len())])
            } else {
                SemanticSource::JsonStringTokenRanges(vec![ByteRange::new(0, invalid.len())])
            };
            let mut window = RawWindow::new();
            window
                .retain(
                    WindowRecord::raw(vec![invalid.clone()]).with_fragments(vec![SemanticFragment { bucket, source }]),
                )
                .unwrap();
            let expected = window.snapshot(bucket).unwrap().materialize().unwrap_err();
            assert_eq!(
                window.resolve(WindowDecision::Replace(Bytes::from_static(b"replacement"))),
                Err(expected)
            );
            assert_eq!(window.resolve(WindowDecision::Discard).unwrap(), WindowOutput::Discard);
            window.start_next().unwrap();
            window
                .retain(WindowRecord::raw(vec![Bytes::from_static(b"next")]))
                .unwrap();
            assert_eq!(released(window.resolve(WindowDecision::Release).unwrap()), b"next");
        }
    }

    #[test]
    fn discard_never_materializes_semantics_or_prepares_overlap() {
        let bucket = key(0, LogicalField::Content);
        let invalid = Bytes::from_static(b"\xff");
        let mut overlap = RawWindow::new();
        overlap
            .retain(
                WindowRecord::raw(vec![invalid.clone()])
                    .with_fragments(vec![fragment(bucket, vec![ByteRange::new(0, 1)])]),
            )
            .unwrap();
        assert_eq!(overlap.resolve(WindowDecision::Discard).unwrap(), WindowOutput::Discard);
        overlap.start_next().unwrap();

        let raw = Bytes::from_static(b"value");
        let mut combined = RawWindow::new();
        combined
            .retain(event(raw.clone(), bucket, vec![ByteRange::new(0, raw.len())]))
            .unwrap();
        combined.limits.combined = combined.raw.resident_capacity();
        assert_eq!(
            combined.snapshot(bucket).unwrap().materialize(),
            Err(WindowError::Limit(WindowLimit::CombinedResidentBytes))
        );
        assert_eq!(
            combined.resolve(WindowDecision::Discard).unwrap(),
            WindowOutput::Discard
        );
        combined.start_next().unwrap();
    }

    #[test]
    fn one_byte_stream_coalesces_once_and_is_linear() {
        let mut limits = Limits::default();
        limits.ranges = usize::MAX;
        limits.lines = usize::MAX;
        limits.events = usize::MAX;
        limits.metadata = usize::MAX;
        limits.combined = usize::MAX;
        let mut window = RawWindow::with_limits(limits);
        let bytes = MAX_SLICE_HANDLES + 10_000;
        for _ in 0..bytes {
            window
                .retain(WindowRecord::raw(vec![Bytes::from_static(b"x")]))
                .unwrap();
        }
        let (_, _, handles, coalesces, copied) = window.accounting();
        assert_eq!(handles, 1);
        assert_eq!(coalesces, 1);
        assert_eq!(copied, bytes);
        assert_eq!(
            released(window.resolve(WindowDecision::Release).unwrap()),
            vec![b'x'; bytes]
        );
    }

    #[test]
    fn one_large_record_bypasses_unbounded_handle_metadata() {
        let count = 10_000;
        let mut window = RawWindow::new();
        window
            .retain(WindowRecord::raw(vec![Bytes::from_static(b"x"); count]))
            .unwrap();
        let (logical, resident, handles, coalesces, copied) = window.accounting();
        assert_eq!(logical, count);
        assert_eq!(handles, 1);
        assert_eq!(coalesces, 1);
        assert_eq!(copied, count);
        assert_eq!(window.raw.slice_metadata_capacity(), 0);
        assert!(resident <= MAX_RESIDENT_BYTES);
        let before_empty_append = window.accounting();
        window.retain(WindowRecord::raw(vec![Bytes::new(); 100_000])).unwrap();
        assert_eq!(window.accounting(), before_empty_append);
        assert_eq!(
            released(window.resolve(WindowDecision::Release).unwrap()),
            vec![b'x'; count]
        );

        let mut empty = RawWindow::new();
        empty.retain(WindowRecord::raw(vec![Bytes::new(); 100_000])).unwrap();
        assert_eq!(empty.accounting(), (0, 0, 0, 0, 0));
        assert_eq!(released(empty.resolve(WindowDecision::Release).unwrap()), b"");
    }

    #[test]
    fn ranges_are_counted_from_actual_fragments_and_failure_is_atomic() {
        let mut limits = Limits::default();
        limits.ranges = 3; // one raw record plus two semantic ranges
        let bucket = key(0, LogicalField::Content);
        let mut window = RawWindow::with_limits(limits);
        window
            .retain(event(
                Bytes::from_static(b"ab"),
                bucket,
                vec![ByteRange::new(0, 1), ByteRange::new(1, 2)],
            ))
            .unwrap();
        let before = window.accounting();
        assert_eq!(
            window.retain(event(Bytes::from_static(b"c"), bucket, vec![ByteRange::new(0, 1)])),
            Err(WindowError::Limit(WindowLimit::RawRanges))
        );
        assert_eq!(window.accounting(), before);
        assert_eq!(released(window.resolve(WindowDecision::Release).unwrap()), b"ab");
    }

    #[test]
    fn count_limits_accept_exact_and_reject_plus_one_atomically() {
        let mut raw_limits = Limits::default();
        raw_limits.raw = 1;
        raw_limits.resident = 1;
        raw_limits.combined = 128 * 1024;
        let mut raw = RawWindow::with_limits(raw_limits);
        raw.retain(WindowRecord::raw(vec![Bytes::from_static(b"x")])).unwrap();
        assert_eq!(
            raw.retain(WindowRecord::raw(vec![Bytes::from_static(b"y")])),
            Err(WindowError::Limit(WindowLimit::RawBytes))
        );
        assert_eq!(released(raw.resolve(WindowDecision::Release).unwrap()), b"x");

        let mut resident_limits = Limits::default();
        resident_limits.resident = 1;
        resident_limits.combined = 1024 * 1024;
        let mut resident = RawWindow::with_limits(resident_limits);
        resident
            .retain(WindowRecord::raw(vec![Bytes::from_static(b"x")]))
            .unwrap();
        assert_eq!(
            resident.retain(WindowRecord::raw(vec![Bytes::from_static(b"y")])),
            Err(WindowError::Limit(WindowLimit::ResidentBytes))
        );
        assert_eq!(released(resident.resolve(WindowDecision::Release).unwrap()), b"x");

        for (line_limit, event_limit, expected) in [
            (1, usize::MAX, WindowLimit::Lines),
            (usize::MAX, 1, WindowLimit::Events),
        ] {
            let mut limits = Limits::default();
            limits.lines = line_limit;
            limits.events = event_limit;
            let mut window = RawWindow::with_limits(limits);
            let record = if expected == WindowLimit::Lines {
                WindowRecord::raw(vec![Bytes::from_static(b"a\n")])
            } else {
                WindowRecord::raw(vec![Bytes::from_static(b"a")]).complete()
            };
            window.retain(record).unwrap();
            let extra = if expected == WindowLimit::Lines {
                WindowRecord::raw(vec![Bytes::from_static(b"b\n")])
            } else {
                WindowRecord::raw(vec![Bytes::from_static(b"b")]).complete()
            };
            assert_eq!(window.retain(extra), Err(WindowError::Limit(expected)));
        }
    }

    #[test]
    fn handle_limit_exact_then_one_time_coalesce() {
        let mut window = RawWindow::new();
        window
            .retain(WindowRecord::raw(vec![Bytes::from_static(b"x"); MAX_SLICE_HANDLES]))
            .unwrap();
        assert_eq!(window.accounting().2, MAX_SLICE_HANDLES);
        assert_eq!(window.accounting().3, 0);
        window
            .retain(WindowRecord::raw(vec![Bytes::from_static(b"x")]))
            .unwrap();
        assert_eq!(window.accounting().2, 1);
        assert_eq!(window.accounting().3, 1);
    }

    #[test]
    fn bucket_fragment_metadata_snapshot_overlap_and_replacement_bounds() {
        let a = key(0, LogicalField::Content);
        let b = key(0, LogicalField::ToolArguments { tool_call_index: 1 });
        let mut limits = Limits::default();
        limits.buckets = 1;
        limits.fragments = 1;
        let mut window = RawWindow::with_limits(limits);
        window
            .retain(event(Bytes::from_static(b"a"), a, vec![ByteRange::new(0, 1)]))
            .unwrap();
        assert_eq!(
            window.retain(event(Bytes::from_static(b"b"), a, vec![ByteRange::new(0, 1)])),
            Err(WindowError::Limit(WindowLimit::FragmentsPerBucket))
        );
        assert_eq!(
            window.retain(event(Bytes::from_static(b"b"), b, vec![ByteRange::new(0, 1)])),
            Err(WindowError::Limit(WindowLimit::Buckets))
        );

        let mut limits = Limits::default();
        limits.snapshot = 3;
        let mut snapshot = RawWindow::with_limits(limits);
        snapshot
            .retain(event(Bytes::from_static(b"four"), a, vec![ByteRange::new(0, 4)]))
            .unwrap();
        assert_eq!(
            snapshot.snapshot(a).unwrap().materialize(),
            Err(WindowError::Limit(WindowLimit::SnapshotBytes))
        );
        assert_eq!(released(snapshot.resolve(WindowDecision::Release).unwrap()), b"four");

        let mut limits = Limits::default();
        limits.snapshot = 4;
        let mut snapshot_exact = RawWindow::with_limits(limits);
        snapshot_exact
            .retain(event(Bytes::from_static(b"four"), a, vec![ByteRange::new(0, 4)]))
            .unwrap();
        assert_eq!(snapshot_exact.snapshot(a).unwrap().materialize().unwrap(), "four");

        let mut limits = Limits::default();
        limits.replacement = 3;
        let mut replacement = RawWindow::with_limits(limits);
        assert_eq!(
            replacement
                .resolve(WindowDecision::Replace(Bytes::from_static(b"123")))
                .unwrap(),
            WindowOutput::Replace(Bytes::from_static(b"123"))
        );
        replacement.start_next().unwrap();
        assert_eq!(
            replacement.resolve(WindowDecision::Replace(Bytes::from_static(b"1234"))),
            Err(WindowError::Limit(WindowLimit::ReplacementBytes))
        );
        assert_eq!(
            replacement.resolve(WindowDecision::Discard).unwrap(),
            WindowOutput::Discard
        );

        let mut limits = Limits::default();
        limits.metadata = 1;
        limits.combined = 1024;
        let mut metadata = RawWindow::with_limits(limits);
        assert_eq!(
            metadata.retain(WindowRecord::raw(vec![Bytes::from_static(b"x")])),
            Err(WindowError::Limit(WindowLimit::MetadataBytes))
        );

        let probe = SliceChain::default()
            .plan_append(vec![Bytes::from_static(b"x")], MAX_RAW_BYTES, MAX_RESIDENT_BYTES)
            .unwrap();
        let exact_metadata = metadata_charge(probe.metadata_capacity(), &Vec::new()).unwrap();
        let mut limits = Limits::default();
        limits.metadata = exact_metadata;
        let mut metadata_exact = RawWindow::with_limits(limits);
        metadata_exact
            .retain(WindowRecord::raw(vec![Bytes::from_static(b"x")]))
            .unwrap();

        let exact_combined = probe.scratch_resident() + exact_metadata;
        let mut limits = Limits::default();
        limits.combined = exact_combined;
        let mut combined_exact = RawWindow::with_limits(limits);
        combined_exact
            .retain(WindowRecord::raw(vec![Bytes::from_static(b"x")]))
            .unwrap();
        let mut limits = Limits::default();
        limits.combined = exact_combined - 1;
        let mut combined_plus_one = RawWindow::with_limits(limits);
        assert_eq!(
            combined_plus_one.retain(WindowRecord::raw(vec![Bytes::from_static(b"x")])),
            Err(WindowError::Limit(WindowLimit::CombinedResidentBytes))
        );
    }

    #[test]
    fn overlap_is_one_global_max_min_utf8_safe_budget() {
        let mut limits = Limits::default();
        limits.overlap = 7;
        let a = key(0, LogicalField::Content);
        let b = key(1, LogicalField::ReasoningContent);
        let mut window = RawWindow::with_limits(limits);
        window
            .retain(event(
                Bytes::from_static("aéé".as_bytes()),
                a,
                vec![ByteRange::new(0, 5)],
            ))
            .unwrap();
        window
            .retain(event(Bytes::from_static(b"123456"), b, vec![ByteRange::new(0, 6)]))
            .unwrap();
        window.resolve(WindowDecision::Release).unwrap();
        window.start_next().unwrap();
        let av = window.snapshot(a).unwrap().materialize().unwrap();
        let bv = window.snapshot(b).unwrap().materialize().unwrap();
        assert_eq!(av, "éé");
        assert_eq!(bv, "456");
        assert!(av.len() + bv.len() <= 7);
    }

    #[test]
    fn configurable_overlap_applies_per_bucket_then_global_fairness() {
        let a = key(0, LogicalField::Content);
        let b = key(1, LogicalField::Content);
        let mut window = RawWindow::with_policy(RawWindowPolicy {
            per_bucket_overlap_bytes: 3,
            global_overlap_bytes: 4,
            max_replacement_bytes: MAX_REPLACEMENT_BYTES,
        })
        .unwrap();
        window
            .retain(event(Bytes::from_static(b"abcdef"), a, vec![ByteRange::new(0, 6)]))
            .unwrap();
        window
            .retain(event(Bytes::from_static(b"123456"), b, vec![ByteRange::new(0, 6)]))
            .unwrap();
        window.resolve(WindowDecision::Release).unwrap();
        window.start_next().unwrap();
        let views = window.snapshot_all().unwrap();
        assert_eq!(views[0].context, "ef");
        assert_eq!(views[1].context, "56");
    }

    #[test]
    fn semantic_replacement_preserves_non_semantic_bytes_and_is_atomic() {
        let key = key(0, LogicalField::Content);
        let raw = Bytes::from_static(
            br#": keep
data: {"choices":[{"index":0,"delta":{"content":"old","metadata":"same"}}]}

"#,
        );
        let token_start = raw.windows(5).position(|part| part == b"\"old\"").unwrap();
        let mut window = RawWindow::new();
        window
            .retain(
                WindowRecord::raw(vec![raw.clone()])
                    .complete()
                    .with_fragments(vec![SemanticFragment {
                        bucket: key,
                        source: SemanticSource::JsonStringTokenRanges(vec![ByteRange::new(
                            token_start,
                            token_start + 5,
                        )]),
                    }]),
            )
            .unwrap();
        assert_eq!(window.stats().unwrap().current_semantic_bytes, 3);
        assert_eq!(window.snapshot_all().unwrap()[0].current, "old");
        assert_eq!(
            window.resolve_semantic_replace(Vec::new()),
            Err(WindowError::InvalidReplacement)
        );
        let output = window
            .resolve_semantic_replace(vec![SemanticReplacement {
                key,
                current: "new\\\"value".into(),
            }])
            .unwrap();
        let WindowOutput::Replace(output) = output else {
            panic!("expected replacement")
        };
        let mut expected = raw[..token_start].to_vec();
        expected.extend_from_slice(br#""new\\\"value""#);
        expected.extend_from_slice(&raw[token_start + 5..]);
        assert_eq!(output.as_ref(), expected);
        window.start_next().unwrap();
        assert_eq!(window.snapshot_all().unwrap()[0].context, "new\\\"value");
    }

    #[test]
    fn failed_try_retain_returns_the_exact_record_for_fresh_generation_retry() {
        let mut limits = Limits::default();
        limits.raw = 1;
        limits.resident = 1;
        let mut window = RawWindow::with_limits(limits);
        window
            .retain(WindowRecord::raw(vec![Bytes::from_static(b"a")]))
            .unwrap();
        let (error, record) = window
            .try_retain(WindowRecord::raw(vec![Bytes::from_static(b"b")]))
            .unwrap_err();
        assert_eq!(error, WindowError::Limit(WindowLimit::RawBytes));
        assert_eq!(record.raw_len(), 1);
        assert_eq!(released(window.resolve(WindowDecision::Release).unwrap()), b"a");
    }

    #[test]
    fn try_retain_charges_original_and_prospective_metadata_exactly() {
        let mut slices = Vec::with_capacity(3);
        slices.push(Bytes::from_static(b"x"));
        let record = WindowRecord::raw(slices);
        let transient = record.metadata_bytes();
        let probe = SliceChain::default()
            .plan_append(record.slices.clone(), MAX_RAW_BYTES, MAX_RESIDENT_BYTES)
            .unwrap();
        let prospective = metadata_charge(probe.metadata_capacity(), &Vec::new()).unwrap();
        let exact = transient.checked_add(prospective).unwrap();

        let mut exact_limits = Limits::default();
        exact_limits.metadata = exact;
        exact_limits.combined = usize::MAX;
        let mut exact_window = RawWindow::with_limits(exact_limits);
        exact_window.try_retain(record.clone()).unwrap();
        assert_eq!(released(exact_window.resolve(WindowDecision::Release).unwrap()), b"x");

        let mut plus_one_limits = Limits::default();
        plus_one_limits.metadata = exact - 1;
        plus_one_limits.combined = usize::MAX;
        let mut plus_one_window = RawWindow::with_limits(plus_one_limits);
        let original_vec_ptr = record.slices.as_ptr();
        let original_bytes_ptr = record.slices[0].as_ptr();
        let original_capacity = record.slices.capacity();
        let (error, returned) = plus_one_window.try_retain(record).unwrap_err();
        assert_eq!(error, WindowError::Limit(WindowLimit::MetadataBytes));
        assert_eq!(returned.slices.as_ptr(), original_vec_ptr);
        assert_eq!(returned.slices[0].as_ptr(), original_bytes_ptr);
        assert_eq!(returned.slices.capacity(), original_capacity);
        assert_eq!(returned.slices[0], Bytes::from_static(b"x"));
        assert_eq!(plus_one_window.accounting().0, 0);
    }

    #[test]
    fn try_retain_many_handles_fails_bounded_and_returns_original_handles() {
        let handle_count = MAX_SLICE_HANDLES + 1;
        let slices = vec![Bytes::from_static(b"x"); handle_count];
        let record = WindowRecord::raw(slices);
        let original_vec_ptr = record.slices.as_ptr();
        let original_handle_ptrs: Vec<_> = record.slices.iter().map(|bytes| bytes.as_ptr()).collect();
        let original_capacity = record.slices.capacity();

        let mut limits = Limits::default();
        limits.metadata = record.metadata_bytes().saturating_sub(1);
        limits.combined = usize::MAX;
        let mut window = RawWindow::with_limits(limits);
        let (error, returned) = window.try_retain(record).unwrap_err();
        assert_eq!(error, WindowError::Limit(WindowLimit::MetadataBytes));
        assert_eq!(returned.slices.as_ptr(), original_vec_ptr);
        assert_eq!(returned.slices.capacity(), original_capacity);
        assert_eq!(returned.slices.len(), handle_count);
        assert!(returned
            .slices
            .iter()
            .zip(original_handle_ptrs)
            .all(|(bytes, pointer)| bytes.as_ptr() == pointer));
        assert_eq!(released(window.resolve(WindowDecision::Release).unwrap()), b"");
    }

    #[test]
    fn replacement_preview_is_non_mutating_and_finish_is_terminal() {
        let key = key(0, LogicalField::Content);
        let raw = Bytes::from_static(
            br#"data: {"choices":[{"index":0,"delta":{"content":"x"}}]}

"#,
        );
        let start = raw.windows(3).position(|part| part == b"\"x\"").unwrap();
        let mut window = RawWindow::new();
        window
            .retain(WindowRecord::raw(vec![raw]).with_fragments(vec![SemanticFragment {
                bucket: key,
                source: SemanticSource::JsonStringTokenRanges(vec![ByteRange::new(start, start + 3)]),
            }]))
            .unwrap();
        let replacement = vec![SemanticReplacement {
            key,
            current: "exact".into(),
        }];
        let preview = window.preview_semantic_replace(&replacement).unwrap();
        assert_eq!(window.snapshot_all().unwrap()[0].current, "x");
        let WindowOutput::Replace(output) = window.finish_semantic_replace(replacement).unwrap() else {
            panic!("expected replacement");
        };
        assert_eq!(preview, output.len());
        assert_eq!(window.start_next(), Err(WindowError::Finished));
    }

    #[test]
    fn zero_text_limit_allows_empty_replacement_without_limiting_encoded_output() {
        let key = key(0, LogicalField::Content);
        let raw = Bytes::from_static(
            br#"data: {"choices":[{"index":0,"delta":{"content":"x"}}]}

"#,
        );
        let start = raw.windows(3).position(|part| part == b"\"x\"").unwrap();
        let mut window = RawWindow::with_policy(RawWindowPolicy {
            per_bucket_overlap_bytes: 0,
            global_overlap_bytes: 0,
            max_replacement_bytes: 0,
        })
        .unwrap();
        window
            .retain(
                WindowRecord::raw(vec![raw.clone()]).with_fragments(vec![SemanticFragment {
                    bucket: key,
                    source: SemanticSource::JsonStringTokenRanges(vec![ByteRange::new(start, start + 3)]),
                }]),
            )
            .unwrap();
        let WindowOutput::Replace(output) = window
            .resolve_semantic_replace(vec![SemanticReplacement {
                key,
                current: String::new(),
            }])
            .unwrap()
        else {
            panic!("expected replacement")
        };
        assert_eq!(output.len(), raw.len().saturating_sub(1));
    }

    #[test]
    fn semantic_rewrite_streams_raw_gaps_across_many_slices() {
        let key = key(0, LogicalField::Content);
        let prefix = Bytes::from(vec![b'a'; 300_000]);
        let token = Bytes::from_static(b"\"x\"");
        let suffix = Bytes::from(vec![b'b'; 300_000]);
        let mut window = RawWindow::new();
        window
            .retain(
                WindowRecord::raw(vec![prefix, token, suffix]).with_fragments(vec![SemanticFragment {
                    bucket: key,
                    source: SemanticSource::JsonStringTokenRanges(vec![ByteRange::new(300_000, 300_003)]),
                }]),
            )
            .unwrap();
        let WindowOutput::Replace(output) = window
            .resolve_semantic_replace(vec![SemanticReplacement {
                key,
                current: "z".into(),
            }])
            .unwrap()
        else {
            panic!("expected replacement")
        };
        assert_eq!(output.len(), 600_003);
        assert_eq!(&output[299_999..300_004], b"a\"z\"b");
    }

    #[test]
    fn lifecycle_models_processor_output_and_terminate_order() {
        #[derive(Debug, PartialEq, Eq)]
        enum Action {
            Current(Vec<u8>),
            Extra(Vec<u8>),
            Terminate,
            Eos,
        }
        let mut actions = Vec::new();
        let mut window = RawWindow::new();
        window
            .retain(WindowRecord::raw(vec![Bytes::from_static(b"held")]))
            .unwrap();
        for bytes in match window.resolve(WindowDecision::Release).unwrap() {
            WindowOutput::Release(chunks) => chunks,
            _ => unreachable!(),
        } {
            actions.push(Action::Current(bytes.to_vec()));
        }
        window.start_next().unwrap();
        window
            .retain(WindowRecord::raw(vec![Bytes::from_static(b"drop")]))
            .unwrap();
        assert_eq!(window.resolve(WindowDecision::Discard).unwrap(), WindowOutput::Discard);
        window.start_next().unwrap();
        window
            .retain(WindowRecord::raw(vec![Bytes::from_static(b"old")]))
            .unwrap();
        if let WindowOutput::Replace(bytes) = window
            .finish(WindowDecision::Replace(Bytes::from_static(b"terminal")))
            .unwrap()
        {
            actions.push(Action::Extra(bytes.to_vec()));
        }
        actions.push(Action::Terminate);
        actions.push(Action::Eos);
        assert_eq!(
            actions,
            vec![
                Action::Current(b"held".to_vec()),
                Action::Extra(b"terminal".to_vec()),
                Action::Terminate,
                Action::Eos
            ]
        );
    }

    #[test]
    fn long_stream_releases_without_generation_growth() {
        let mut window = RawWindow::new();
        for i in 0..10_000 {
            let raw = Bytes::from(format!("data:{i}\n\n"));
            window
                .retain(WindowRecord::raw(
                    raw.as_ref()
                        .iter()
                        .map(|byte| Bytes::copy_from_slice(&[*byte]))
                        .collect(),
                ))
                .unwrap();
            assert_eq!(released(window.resolve(WindowDecision::Release).unwrap()), raw);
            window.start_next().unwrap();
            assert_eq!(window.accounting().0, 0);
        }
    }
}
