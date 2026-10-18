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
    /// Binary framing has no SSE lines or semantic ranges.
    pub(crate) fn binary(bytes: Bytes) -> Self {
        Self {
            slices: vec![bytes],
            line_count: 0,
            complete_event: false,
            semantic_available: true,
            fragments: Vec::new(),
        }
    }

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

    pub(crate) fn raw_slices_cloned(&self) -> impl IntoIterator<Item = Bytes> + '_ {
        self.slices.iter().cloned()
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

#[derive(Debug)]
struct StoredFragment {
    source: SemanticSource,
}

#[derive(Debug)]
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
    // Owned bucket/fragment/range capacities, updated on commit. Avoid walking
    // every retained range again on each single-record append.
    bucket_metadata: usize,
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
            bucket_metadata: 0,
            semantic_degraded: false,
            semantic_failure: None,
            state: State::Collecting,
            policy,
        }
    }

    /// Materialize one admitted binary message, never an entire response.
    pub(crate) fn binary_bytes(&self) -> Result<Vec<u8>, WindowError> {
        self.require_collecting()?;
        self.admit(self.raw.len())?;
        let mut output = Vec::with_capacity(self.raw.len());
        self.raw
            .for_each_range(super::chain::ByteRange::new(0, self.raw.len()), |part| {
                output.extend_from_slice(part)
            });
        Ok(output)
    }

    pub fn stats(&mut self) -> Result<WindowStats, WindowError> {
        self.require_collecting()?;
        let scan = self.scan(0).inspect_err(|error| {
            self.semantic_degraded = true;
            self.semantic_failure = Some(*error);
        })?;
        Ok(WindowStats {
            raw_bytes: self.raw.len(),
            raw_ranges: self.raw_ranges,
            events: self.events,
            lines: self.lines,
            current_semantic_bytes: checked_sum(scan.lengths).ok_or(WindowError::Limit(WindowLimit::SnapshotBytes))?,
        })
    }

    pub fn snapshot_all(&mut self) -> Result<Vec<SemanticWindow>, WindowError> {
        self.require_collecting()?;
        if self.semantic_degraded {
            return Err(WindowError::SemanticUnavailable);
        }
        self.materialize_all().inspect_err(|error| {
            self.semantic_degraded = true;
            self.semantic_failure = Some(*error);
        })
    }

    fn materialize_all(&self) -> Result<Vec<SemanticWindow>, WindowError> {
        let mut scan = self.scan(0)?;
        let mut output_bytes = allocation_bytes::<SemanticWindow>(self.buckets.len())?;
        self.admit_metadata(output_bytes)?;
        for (index, bucket) in self.buckets.iter().enumerate() {
            output_bytes = add(output_bytes, add(scan.lengths[index], bucket.overlap.len())?)?;
        }
        check_limit(output_bytes, self.limits.snapshot, WindowLimit::SnapshotBytes)?;
        self.admit(add(output_bytes, scan.scratch.capacity())?)?;
        let mut output = Vec::with_capacity(self.buckets.len());
        for (index, bucket) in self.buckets.iter().enumerate() {
            let mut current = String::with_capacity(scan.lengths[index]);
            self.visit_bucket(bucket, &mut scan.scratch, |text| {
                current.push_str(text);
                Ok(())
            })?;
            output.push(SemanticWindow {
                key: bucket.key,
                context: bucket.overlap.clone(),
                current,
            });
        }
        Ok(output)
    }

    pub fn resolve_semantic_replace(
        &mut self,
        replacements: Vec<SemanticReplacement>,
    ) -> Result<WindowOutput, WindowError> {
        self.apply_semantic_replace(replacements, false)
    }

    /// Allocation-free encoded length only. Successful application produces
    /// this length, but must still admit its edit/output/context allocations.
    pub fn preview_semantic_replace<'a>(
        &self,
        replacements: impl IntoIterator<Item = (LogicalBucketKey, &'a str)>,
    ) -> Result<usize, WindowError> {
        self.require_collecting()?;
        if let Some(error) = self.semantic_failure {
            return Err(error);
        }
        let texts = self.replacement_texts(replacements)?;
        Ok(self.replacement_size(&texts)?.0)
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
        let prepared = self.prepare_semantic_replace(replacements, finish)?;
        if let Some(overlaps) = prepared.overlaps {
            self.install_overlaps(overlaps);
        }
        self.raw.clear();
        self.complete_generation(finish);
        Ok(WindowOutput::Replace(prepared.output))
    }

    /// Retain a record atomically. Admission borrows the input; failure leaves
    /// both the previous generation and a try_retain input untouched.
    pub fn retain(&mut self, record: WindowRecord) -> Result<(), WindowError> {
        self.try_retain(record).map_err(|(error, _)| error)
    }

    pub fn try_retain(&mut self, record: WindowRecord) -> Result<(), (WindowError, WindowRecord)> {
        let admitted = (|| {
            self.require_collecting()?;
            let raw = self
                .raw
                .inspect_append(&record.slices, self.limits.raw, self.limits.resident)
                .map_err(|limit| match limit {
                    ChainLimit::Logical => WindowError::Limit(WindowLimit::RawBytes),
                    ChainLimit::Resident => WindowError::Limit(WindowLimit::ResidentBytes),
                })?;
            check_limit(raw.handles, MAX_SLICE_HANDLES, WindowLimit::SliceHandles)?;
            check_limit(raw.metadata, self.limits.metadata, WindowLimit::MetadataBytes)?;
            let buckets = BucketAppend::inspect(
                &self.buckets,
                self.bucket_metadata,
                &record,
                self.raw.len(),
                self.limits,
            )?;
            let ranges = add(self.raw_ranges, add(1, buckets.ranges)?)?;
            let lines = add(self.lines, record.line_count)?;
            let events = add(self.events, usize::from(record.complete_event))?;
            check_limit(ranges, self.limits.ranges, WindowLimit::RawRanges)?;
            check_limit(lines, self.limits.lines, WindowLimit::Lines)?;
            check_limit(events, self.limits.events, WindowLimit::Events)?;
            let metadata = add(
                add(self.raw.slice_metadata_capacity(), self.bucket_metadata)?,
                add(
                    record.metadata_bytes(),
                    add(raw.additional_metadata, buckets.additional_metadata)?,
                )?,
            )?;
            check_limit(metadata, self.limits.metadata, WindowLimit::MetadataBytes)?;
            self.admit(add(
                record.metadata_bytes(),
                add(
                    raw.additional_resident,
                    add(raw.additional_metadata, buckets.additional_metadata)?,
                )?,
            )?)?;
            Ok((raw, buckets, ranges, lines, events))
        })();
        let (raw, buckets, ranges, lines, events) = match admitted {
            Ok(plan) => plan,
            Err(error) => return Err((error, record)),
        };
        // All limits and sizes are known. Preparing these exact-capacity
        // containers and moving admitted elements cannot fail validation.
        let base = self.raw.len();
        let bucket_metadata = buckets.metadata;
        let buckets = buckets.prepare();
        let raw = self.raw.prepare_append(record.slices, raw);
        buckets.commit(&mut self.buckets, record.fragments, base);
        self.raw.commit_append(raw);
        self.bucket_metadata = bucket_metadata;
        self.raw_ranges = ranges;
        self.lines = lines;
        self.events = events;
        self.semantic_degraded |= !record.semantic_available;
        if !record.semantic_available {
            self.semantic_failure = Some(WindowError::SemanticUnavailable);
        }
        Ok(())
    }

    fn resident(&self) -> Result<usize, WindowError> {
        add(
            self.raw.resident_capacity(),
            add(
                overlap_capacity(&self.buckets)?,
                add(self.raw.slice_metadata_capacity(), self.bucket_metadata)?,
            )?,
        )
    }

    /// Admit additional visible capacities before constructing them. Payload
    /// ownership transfers are counted once; allocator control blocks and
    /// caller-owned Bytes backing are outside this capacity contract.
    fn admit(&self, additional: usize) -> Result<(), WindowError> {
        check_limit(
            add(self.resident()?, additional)?,
            self.limits.combined,
            WindowLimit::CombinedResidentBytes,
        )
    }

    fn admit_metadata(&self, additional: usize) -> Result<(), WindowError> {
        check_limit(
            add(
                add(self.raw.slice_metadata_capacity(), self.bucket_metadata)?,
                additional,
            )?,
            self.limits.metadata,
            WindowLimit::MetadataBytes,
        )
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

    /// Resolve exactly once. Semantic failure permits a byte-identical raw
    /// release, but breaks semantic continuity across the unobserved output.
    pub fn resolve(&mut self, decision: WindowDecision) -> Result<WindowOutput, WindowError> {
        self.apply_decision(decision, false)
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
        self.apply_decision(decision, true)
    }

    fn apply_decision(&mut self, decision: WindowDecision, finish: bool) -> Result<WindowOutput, WindowError> {
        self.require_collecting()?;
        if matches!(decision, WindowDecision::Discard) {
            self.raw.clear();
            self.complete_generation(finish);
            return Ok(WindowOutput::Discard);
        }
        let release = matches!(decision, WindowDecision::Release);
        if let Some(error) = self.semantic_failure {
            if !release {
                return Err(error);
            }
        }
        let prepared = self.prepare_decision(decision)?;
        if !finish {
            if release && !self.semantic_degraded {
                let output_metadata = allocation_bytes::<Bytes>(self.raw.handle_count())?;
                let overlaps = self.prepare_overlaps(None, output_metadata).inspect_err(|error| {
                    self.semantic_degraded = true;
                    self.semantic_failure = Some(*error);
                })?;
                self.install_overlaps(overlaps);
            } else {
                // Opaque output cannot be associated with logical buckets.
                for bucket in &mut self.buckets {
                    bucket.overlap = String::new();
                }
            }
        }
        let output = match prepared {
            PreparedDecision::Release(output) => WindowOutput::Release(self.raw.take_raw_slices_into(output)),
            PreparedDecision::Replace(bytes) => {
                self.raw.clear();
                WindowOutput::Replace(bytes)
            }
        };
        self.complete_generation(finish);
        Ok(output)
    }

    fn complete_generation(&mut self, finish: bool) {
        self.clear_generation();
        if finish {
            self.buckets = Vec::new();
            self.bucket_metadata = 0;
            self.state = State::Finished;
        } else {
            for bucket in &mut self.buckets {
                bucket.fragments.clear();
            }
            // Ranges have been dropped; only admitted container capacity remains.
            self.bucket_metadata = self.buckets.capacity() * size_of::<Bucket>()
                + self
                    .buckets
                    .iter()
                    .map(|bucket| bucket.fragments.capacity() * size_of::<StoredFragment>())
                    .sum::<usize>();
            self.state = State::Resolved;
        }
    }

    fn prepare_decision(&self, decision: WindowDecision) -> Result<PreparedDecision, WindowError> {
        match decision {
            WindowDecision::Release => {
                let metadata = allocation_bytes::<Bytes>(self.raw.handle_count())?;
                self.admit_metadata(metadata)?;
                self.admit(metadata)?;
                Ok(PreparedDecision::Release(Vec::with_capacity(self.raw.handle_count())))
            }
            WindowDecision::Replace(bytes) => {
                check_limit(bytes.len(), self.limits.replacement, WindowLimit::ReplacementBytes)?;
                self.admit(bytes.len())?;
                let (bytes, _) = normalize_replacement(bytes, self.limits.replacement)?;
                Ok(PreparedDecision::Replace(bytes))
            }
            WindowDecision::Discard => Err(WindowError::InvalidReplacement),
        }
    }

    fn token_scratch(&self, buckets: &[Bucket], extra: usize) -> Result<Vec<u8>, WindowError> {
        let mut length = 0;
        for bucket in buckets {
            for fragment in &bucket.fragments {
                length = length.max(
                    checked_sum(fragment.source.ranges().iter().map(|range| range.len()))
                        .ok_or(WindowError::Limit(WindowLimit::SnapshotBytes))?,
                );
            }
        }
        self.admit(add(extra, length)?)?;
        Ok(Vec::with_capacity(length))
    }

    fn visit_bucket(
        &self,
        bucket: &Bucket,
        scratch: &mut Vec<u8>,
        mut visitor: impl FnMut(&str) -> Result<(), WindowError>,
    ) -> Result<(), WindowError> {
        for fragment in &bucket.fragments {
            scratch.clear();
            for &range in fragment.source.ranges() {
                self.raw
                    .for_each_range(range, |part| scratch.extend_from_slice(part))
                    .ok_or(WindowError::InvalidRange)?;
            }
            match &fragment.source {
                SemanticSource::Utf8Ranges(_) => {
                    visitor(std::str::from_utf8(scratch).map_err(|_| WindowError::InvalidUtf8)?)?
                }
                SemanticSource::JsonStringTokenRanges(_) => visit_json_string(scratch, &mut visitor)?,
            }
        }
        Ok(())
    }

    fn scan(&self, extra: usize) -> Result<SemanticScan, WindowError> {
        let mut scan = SemanticScan {
            scratch: self.token_scratch(&self.buckets, extra)?,
            lengths: [0; MAX_BUCKETS],
        };
        let mut total = 0;
        for (index, bucket) in self.buckets.iter().enumerate() {
            self.visit_bucket(bucket, &mut scan.scratch, |text| {
                scan.lengths[index] = add(scan.lengths[index], text.len())?;
                Ok(())
            })?;
            total = add(total, scan.lengths[index])?;
        }
        check_limit(total, self.limits.snapshot, WindowLimit::SnapshotBytes)?;
        Ok(scan)
    }

    fn materialize_bucket(&self, index: usize) -> Result<String, WindowError> {
        let bucket = &self.buckets[index];
        let mut scratch = self.token_scratch(std::slice::from_ref(bucket), 0)?;
        let mut length = bucket.overlap.len();
        self.visit_bucket(bucket, &mut scratch, |text| {
            length = add(length, text.len())?;
            Ok(())
        })?;
        check_limit(length, self.limits.snapshot, WindowLimit::SnapshotBytes)?;
        self.admit(add(length, scratch.capacity())?)?;
        let mut output = String::with_capacity(length);
        output.push_str(&bucket.overlap);
        self.visit_bucket(bucket, &mut scratch, |text| {
            output.push_str(text);
            Ok(())
        })?;
        Ok(output)
    }

    fn prepare_overlaps(
        &self,
        replacements: Option<&[&str; MAX_BUCKETS]>,
        extra: usize,
    ) -> Result<[String; MAX_BUCKETS], WindowError> {
        let mut scan = match replacements {
            Some(texts) => SemanticScan {
                scratch: Vec::new(),
                lengths: std::array::from_fn(|index| texts[index].len()),
            },
            None => self.scan(extra)?,
        };
        let mut needs = [0; MAX_BUCKETS];
        let mut totals = [0; MAX_BUCKETS];
        for (index, bucket) in self.buckets.iter().enumerate() {
            totals[index] = add(bucket.overlap.len(), scan.lengths[index])?;
            needs[index] = totals[index].min(self.policy.per_bucket_overlap_bytes);
        }
        let grants = max_min_allocations(&needs[..self.buckets.len()], self.policy.global_overlap_bytes);
        let capacity = checked_sum(grants).ok_or(WindowError::Limit(WindowLimit::OverlapBytes))?;
        self.admit(add(extra, add(scan.scratch.capacity(), capacity)?)?)?;
        let mut output: [String; MAX_BUCKETS] = std::array::from_fn(|_| String::new());
        for (index, bucket) in self.buckets.iter().enumerate() {
            let mut remaining = totals[index] - grants[index];
            output[index] = String::with_capacity(grants[index]);
            let mut append = |text: &str| {
                let mut start = remaining.min(text.len());
                remaining -= start;
                while start < text.len() && !text.is_char_boundary(start) {
                    start += 1;
                }
                output[index].push_str(&text[start..]);
                Ok(())
            };
            append(&bucket.overlap)?;
            if let Some(texts) = replacements {
                append(texts[index])?;
            } else {
                self.visit_bucket(bucket, &mut scan.scratch, append)?;
            }
        }
        Ok(output)
    }

    fn install_overlaps(&mut self, overlaps: [String; MAX_BUCKETS]) {
        for (bucket, overlap) in self.buckets.iter_mut().zip(overlaps) {
            bucket.overlap = overlap;
        }
    }

    fn replacement_texts<'a>(
        &self,
        replacements: impl IntoIterator<Item = (LogicalBucketKey, &'a str)>,
    ) -> Result<[&'a str; MAX_BUCKETS], WindowError> {
        let mut texts = [""; MAX_BUCKETS];
        let mut seen = [false; MAX_BUCKETS];
        let mut total = 0;
        for (key, text) in replacements {
            let index = self
                .buckets
                .binary_search_by_key(&key, |bucket| bucket.key)
                .map_err(|_| WindowError::InvalidReplacement)?;
            if seen[index] || (self.buckets[index].fragments.is_empty() && !text.is_empty()) {
                return Err(WindowError::InvalidReplacement);
            }
            seen[index] = true;
            texts[index] = text;
            total = add(total, text.len())?;
        }
        if seen[..self.buckets.len()].iter().any(|seen| !seen) {
            return Err(WindowError::InvalidReplacement);
        }
        check_limit(total, self.policy.max_replacement_bytes, WindowLimit::ReplacementBytes)?;
        Ok(texts)
    }

    fn replacement_size(&self, texts: &[&str; MAX_BUCKETS]) -> Result<(usize, usize), WindowError> {
        let mut length = self.raw.len();
        let mut count = 0;
        for (bucket_index, bucket) in self.buckets.iter().enumerate() {
            for (index, fragment) in bucket.fragments.iter().enumerate() {
                let SemanticSource::JsonStringTokenRanges(ranges) = &fragment.source else {
                    return Err(WindowError::InvalidReplacement);
                };
                if ranges.len() != 1 {
                    return Err(WindowError::InvalidReplacement);
                }
                let text = if index == 0 { texts[bucket_index] } else { "" };
                let mut writer = CountWriter(0);
                serde_json::to_writer(&mut writer, text).map_err(|_| WindowError::InvalidReplacement)?;
                length = add(
                    length.checked_sub(ranges[0].len()).ok_or(WindowError::InvalidRange)?,
                    writer.0,
                )?;
                count = add(count, 1)?;
            }
        }
        check_limit(length, self.limits.replacement, WindowLimit::ReplacementBytes)?;
        Ok((length, count))
    }

    fn prepare_semantic_replace(
        &self,
        replacements: Vec<SemanticReplacement>,
        finish: bool,
    ) -> Result<PreparedSemanticReplace, WindowError> {
        let texts = self.replacement_texts(replacements.iter().map(|entry| (entry.key, entry.current.as_str())))?;
        let (output_len, count) = self.replacement_size(&texts)?;
        let input = add(
            allocation_bytes::<SemanticReplacement>(replacements.capacity())?,
            checked_sum(replacements.iter().map(|entry| entry.current.capacity()))
                .ok_or(WindowError::Limit(WindowLimit::ReplacementBytes))?,
        )?;
        let edit_bytes = allocation_bytes::<ReplacementEdit>(count)?;
        self.admit_metadata(add(
            allocation_bytes::<SemanticReplacement>(replacements.capacity())?,
            edit_bytes,
        )?)?;
        let extra = add(input, add(edit_bytes, output_len)?)?;
        self.admit(extra)?;
        let mut edits = Vec::with_capacity(count);
        for (bucket_index, bucket) in self.buckets.iter().enumerate() {
            for (index, fragment) in bucket.fragments.iter().enumerate() {
                edits.push(ReplacementEdit {
                    range: fragment.source.ranges()[0],
                    bucket: bucket_index,
                    first: index == 0,
                });
            }
        }
        edits.sort_unstable_by_key(|edit| edit.range.start);
        if edits.windows(2).any(|pair| pair[0].range.end > pair[1].range.start) {
            return Err(WindowError::InvalidRange);
        }
        let overlaps = if finish {
            None
        } else {
            Some(self.prepare_overlaps(Some(&texts), extra)?)
        };
        let mut output = BytesMut::with_capacity(output_len);
        let mut cursor = 0;
        for edit in edits {
            self.raw
                .for_each_range(ByteRange::new(cursor, edit.range.start), |part| {
                    output.extend_from_slice(part)
                })
                .ok_or(WindowError::InvalidRange)?;
            let text = if edit.first { texts[edit.bucket] } else { "" };
            serde_json::to_writer(OutputWriter(&mut output), text).map_err(|_| WindowError::InvalidReplacement)?;
            cursor = edit.range.end;
        }
        self.raw
            .for_each_range(ByteRange::new(cursor, self.raw.len()), |part| {
                output.extend_from_slice(part)
            })
            .ok_or(WindowError::InvalidRange)?;
        Ok(PreparedSemanticReplace {
            output: output.freeze(),
            overlaps,
        })
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
}

struct PreparedSemanticReplace {
    output: Bytes,
    overlaps: Option<[String; MAX_BUCKETS]>,
}

struct SemanticScan {
    scratch: Vec<u8>,
    lengths: [usize; MAX_BUCKETS],
}

struct ReplacementEdit {
    range: ByteRange,
    bucket: usize,
    first: bool,
}

struct CountWriter(usize);

impl std::io::Write for CountWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0 = self
            .0
            .checked_add(bytes.len())
            .ok_or(std::io::ErrorKind::InvalidInput)?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

struct OutputWriter<'a>(&'a mut BytesMut);

impl std::io::Write for OutputWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.0.capacity() - self.0.len() {
            return Err(std::io::ErrorKind::WriteZero.into());
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct BucketGrowth {
    key: LogicalBucketKey,
    count: usize,
    capacity: usize,
    old_capacity: usize,
}

struct BucketAppend {
    entries: [BucketGrowth; MAX_BUCKETS],
    len: usize,
    outer_capacity: usize,
    grow_outer: bool,
    ranges: usize,
    additional_metadata: usize,
    metadata: usize,
}

struct PreparedBuckets {
    plan: BucketAppend,
    outer: Option<Vec<Bucket>>,
    fragments: [Option<Vec<StoredFragment>>; MAX_BUCKETS],
}

impl BucketAppend {
    fn inspect(
        current: &Vec<Bucket>,
        current_metadata: usize,
        record: &WindowRecord,
        base: usize,
        limits: Limits,
    ) -> Result<Self, WindowError> {
        let empty = BucketGrowth {
            key: LogicalBucketKey {
                choice_index: 0,
                field: LogicalField::Content,
            },
            count: 0,
            capacity: 0,
            old_capacity: 0,
        };
        let mut plan = Self {
            entries: [empty; MAX_BUCKETS],
            len: current.len(),
            outer_capacity: current.capacity(),
            grow_outer: false,
            ranges: 0,
            additional_metadata: 0,
            metadata: current_metadata,
        };
        for (entry, bucket) in plan.entries.iter_mut().zip(current) {
            *entry = BucketGrowth {
                key: bucket.key,
                count: bucket.fragments.len(),
                capacity: bucket.fragments.capacity(),
                old_capacity: bucket.fragments.capacity(),
            };
        }
        let record_len = record.raw_len();
        for fragment in &record.fragments {
            plan.metadata = add(
                plan.metadata,
                allocation_bytes::<ByteRange>(fragment.source.range_capacity())?,
            )?;
            if fragment.source.ranges().is_empty() {
                return Err(WindowError::InvalidRange);
            }
            let mut previous_end = 0;
            for range in fragment.source.ranges() {
                if range.start > range.end || range.end > record_len || range.start < previous_end {
                    return Err(WindowError::InvalidRange);
                }
                add(base, range.end)?;
                previous_end = range.end;
                plan.ranges = add(plan.ranges, 1)?;
            }
            let index = match plan.entries[..plan.len].binary_search_by_key(&fragment.bucket, |entry| entry.key) {
                Ok(index) => index,
                Err(index) => {
                    check_limit(plan.len + 1, limits.buckets.min(MAX_BUCKETS), WindowLimit::Buckets)?;
                    plan.entries.copy_within(index..plan.len, index + 1);
                    plan.entries[index] = BucketGrowth {
                        key: fragment.bucket,
                        ..empty
                    };
                    plan.len += 1;
                    index
                }
            };
            plan.entries[index].count = add(plan.entries[index].count, 1)?;
            check_limit(
                plan.entries[index].count,
                limits.fragments,
                WindowLimit::FragmentsPerBucket,
            )?;
        }
        plan.outer_capacity = grow_capacity(current.capacity(), plan.len, MAX_BUCKETS);
        plan.grow_outer = plan.outer_capacity > current.capacity();
        if plan.grow_outer {
            plan.additional_metadata = allocation_bytes::<Bucket>(plan.outer_capacity)?;
            plan.metadata = add(
                plan.metadata,
                allocation_bytes::<Bucket>(plan.outer_capacity - current.capacity())?,
            )?;
        }
        for entry in &mut plan.entries[..plan.len] {
            entry.capacity = grow_capacity(entry.capacity, entry.count, limits.fragments);
            if entry.capacity > entry.old_capacity {
                plan.metadata = add(
                    plan.metadata,
                    allocation_bytes::<StoredFragment>(entry.capacity - entry.old_capacity)?,
                )?;
                plan.additional_metadata = add(
                    plan.additional_metadata,
                    allocation_bytes::<StoredFragment>(entry.capacity)?,
                )?;
            }
        }
        Ok(plan)
    }

    fn prepare(self) -> PreparedBuckets {
        let outer = self.grow_outer.then(|| Vec::with_capacity(self.outer_capacity));
        let fragments = std::array::from_fn(|index| {
            (index < self.len && self.entries[index].capacity > self.entries[index].old_capacity)
                .then(|| Vec::with_capacity(self.entries[index].capacity))
        });
        PreparedBuckets {
            plan: self,
            outer,
            fragments,
        }
    }
}

impl PreparedBuckets {
    fn commit(mut self, buckets: &mut Vec<Bucket>, fragments: Vec<SemanticFragment>, base: usize) {
        if let Some(mut outer) = self.outer {
            outer.append(buckets);
            *buckets = outer;
        }
        for (index, entry) in self.plan.entries[..self.plan.len].iter().enumerate() {
            if buckets.get(index).is_none_or(|bucket| bucket.key != entry.key) {
                buckets.insert(
                    index,
                    Bucket {
                        key: entry.key,
                        fragments: Vec::new(),
                        overlap: String::new(),
                    },
                );
            }
            if let Some(mut next) = self.fragments[index].take() {
                next.append(&mut buckets[index].fragments);
                buckets[index].fragments = next;
            }
        }
        for mut fragment in fragments {
            for range in fragment.source.ranges_mut() {
                range.start += base;
                range.end += base;
            }
            let index = buckets.partition_point(|bucket| bucket.key < fragment.bucket);
            buckets[index].fragments.push(StoredFragment {
                source: fragment.source,
            });
        }
    }
}

fn grow_capacity(current: usize, needed: usize, maximum: usize) -> usize {
    if needed <= current {
        current
    } else {
        needed.checked_next_power_of_two().unwrap_or(needed).min(maximum)
    }
}

fn add(a: usize, b: usize) -> Result<usize, WindowError> {
    a.checked_add(b)
        .ok_or(WindowError::Limit(WindowLimit::CombinedResidentBytes))
}

fn allocation_bytes<T>(count: usize) -> Result<usize, WindowError> {
    count
        .checked_mul(size_of::<T>())
        .ok_or(WindowError::Limit(WindowLimit::MetadataBytes))
}

fn max_min_allocations(needs: &[usize], budget: usize) -> [usize; MAX_BUCKETS] {
    let mut allocations = [0; MAX_BUCKETS];
    let mut remaining = budget;
    loop {
        let active = needs
            .iter()
            .zip(&allocations)
            .filter(|(need, given)| need > given)
            .count();
        if active == 0 || remaining == 0 {
            break;
        }
        let share = remaining / active;
        let remainder = remaining % active;
        let mut rank = 0;
        let mut used = 0;
        for (index, &need) in needs.iter().enumerate() {
            if allocations[index] == need {
                continue;
            }
            let granted = (share + usize::from(rank < remainder)).min(need - allocations[index]);
            allocations[index] += granted;
            used += granted;
            rank += 1;
        }
        if used == 0 {
            break;
        }
        remaining -= used;
    }
    allocations
}

/// Visit a complete JSON string without decoder-owned heap scratch. Raw token
/// assembly is explicit in the caller; decoded scalars use a four-byte stack
/// buffer. Counting and materialization share this exact validation path.
fn visit_json_string(
    input: &[u8],
    visitor: &mut impl FnMut(&str) -> Result<(), WindowError>,
) -> Result<(), WindowError> {
    let invalid = WindowError::InvalidJsonString;
    let whitespace = |byte: &u8| matches!(byte, b' ' | b'\t' | b'\r' | b'\n');
    let start = input.iter().position(|b| !whitespace(b)).ok_or(invalid)?;
    let end = input.iter().rposition(|b| !whitespace(b)).ok_or(invalid)?;
    if end <= start || input[start] != b'"' || input[end] != b'"' {
        return Err(invalid);
    }
    let body = &input[start + 1..end];
    let mut cursor = 0;
    while cursor < body.len() {
        let start = cursor;
        while cursor < body.len() && body[cursor] != b'\\' {
            if body[cursor] == b'"' || body[cursor] < 0x20 {
                return Err(invalid);
            }
            cursor += 1;
        }
        visitor(std::str::from_utf8(&body[start..cursor]).map_err(|_| invalid)?)?;
        if cursor == body.len() {
            break;
        }
        cursor += 1;
        let escape = *body.get(cursor).ok_or(invalid)?;
        cursor += 1;
        let scalar = match escape {
            b'"' => '"',
            b'\\' => '\\',
            b'/' => '/',
            b'b' => '\x08',
            b'f' => '\x0c',
            b'n' => '\n',
            b'r' => '\r',
            b't' => '\t',
            b'u' => {
                let first = json_hex_quad(body.get(cursor..cursor + 4).ok_or(invalid)?).ok_or(invalid)?;
                cursor += 4;
                let code = if (0xd800..=0xdbff).contains(&first) {
                    if body.get(cursor..cursor + 2) != Some(b"\\u") {
                        return Err(invalid);
                    }
                    cursor += 2;
                    let second = json_hex_quad(body.get(cursor..cursor + 4).ok_or(invalid)?).ok_or(invalid)?;
                    cursor += 4;
                    if !(0xdc00..=0xdfff).contains(&second) {
                        return Err(invalid);
                    }
                    0x10000 + ((first as u32 - 0xd800) << 10) + (second as u32 - 0xdc00)
                } else {
                    first as u32
                };
                char::from_u32(code).ok_or(invalid)?
            }
            _ => return Err(invalid),
        };
        visitor(scalar.encode_utf8(&mut [0; 4]))?;
    }
    Ok(())
}

fn json_hex_quad(bytes: &[u8]) -> Option<u16> {
    bytes.iter().try_fold(0u16, |value, &byte| {
        let digit = match byte {
            b'0'..=b'9' => byte - b'0',
            b'a'..=b'f' => byte - b'a' + 10,
            b'A'..=b'F' => byte - b'A' + 10,
            _ => return None,
        };
        Some((value << 4) | u16::from(digit))
    })
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
#[path = "../../tests/unit/sse/window_test.rs"]
mod tests;
