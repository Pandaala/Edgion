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
    fn one_large_record_coalesces_without_unbounded_retained_metadata() {
        let count = MAX_SLICE_HANDLES + 1;
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
        window.retain(WindowRecord::raw(vec![Bytes::new(); count])).unwrap();
        assert_eq!(window.accounting(), before_empty_append);
        assert_eq!(
            released(window.resolve(WindowDecision::Release).unwrap()),
            vec![b'x'; count]
        );

        let mut empty = RawWindow::new();
        empty.retain(WindowRecord::raw(vec![Bytes::new(); count])).unwrap();
        assert_eq!(empty.accounting(), (0, 0, 0, 0, 0));
        assert_eq!(released(empty.resolve(WindowDecision::Release).unwrap()), b"");
    }

    #[test]
    fn oversized_input_handle_tables_are_rejected_without_mutation() {
        for bytes in [Bytes::from_static(b"x"), Bytes::new()] {
            let mut window = RawWindow::new();
            window
                .retain(WindowRecord::raw(vec![Bytes::from_static(b"prior")]))
                .unwrap();
            let record = WindowRecord::raw(vec![bytes; 10_000]);
            let pointer = record.slices.as_ptr();
            let (error, record) = window.try_retain(record).unwrap_err();
            assert_eq!(error, WindowError::Limit(WindowLimit::MetadataBytes));
            assert_eq!(record.slices.as_ptr(), pointer);
            assert_eq!(released(window.finish(WindowDecision::Release).unwrap()), b"prior");
        }
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
            .inspect_append(&[Bytes::from_static(b"x")], MAX_RAW_BYTES, MAX_RESIDENT_BYTES)
            .unwrap();
        let exact_metadata = probe.metadata + size_of::<Bytes>();
        let mut limits = Limits::default();
        limits.metadata = exact_metadata;
        let mut metadata_exact = RawWindow::with_limits(limits);
        metadata_exact
            .retain(WindowRecord::raw(vec![Bytes::from_static(b"x")]))
            .unwrap();

        let exact_combined = probe.additional_resident + exact_metadata;
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
            .inspect_append(&record.slices, MAX_RAW_BYTES, MAX_RESIDENT_BYTES)
            .unwrap();
        let prospective = probe.additional_metadata;
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
        let preview = window
            .preview_semantic_replace(replacement.iter().map(|entry| (entry.key, entry.current.as_str())))
            .unwrap();
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

    fn json_record(bucket: LogicalBucketKey, text: &str) -> WindowRecord {
        let raw = Bytes::from(serde_json::to_vec(text).unwrap());
        let end = raw.len();
        WindowRecord::raw(vec![raw]).with_fragments(vec![SemanticFragment {
            bucket,
            source: SemanticSource::JsonStringTokenRanges(vec![ByteRange::new(0, end)]),
        }])
    }

    // Independently enumerate currently owned capacities for boundary tests;
    // do not ask the production admission helper for the expected threshold.
    fn owned_capacity(window: &RawWindow) -> usize {
        window.raw.resident_capacity()
            + window.raw.slice_metadata_capacity()
            + window.buckets.capacity() * size_of::<Bucket>()
            + window
                .buckets
                .iter()
                .map(|bucket| {
                    bucket.overlap.capacity()
                        + bucket.fragments.capacity() * size_of::<StoredFragment>()
                        + bucket
                            .fragments
                            .iter()
                            .map(|fragment| fragment.source.range_capacity() * size_of::<ByteRange>())
                            .sum::<usize>()
                })
                .sum::<usize>()
    }

    #[test]
    fn next_context_contains_only_emitted_text_across_three_generations() {
        let bucket = key(0, LogicalField::Content);
        for (action, expected) in [
            ("replace", "abéX"),
            ("empty", "abé"),
            ("discard", "abé"),
            ("opaque", ""),
        ] {
            let mut window = RawWindow::new();
            window.retain(json_record(bucket, "abé")).unwrap();
            window.resolve(WindowDecision::Release).unwrap();
            assert_eq!(window.resident().unwrap(), owned_capacity(&window));
            window.start_next().unwrap();
            window.retain(json_record(bucket, "hidden")).unwrap();
            match action {
                "replace" | "empty" => {
                    window
                        .resolve_semantic_replace(vec![SemanticReplacement {
                            key: bucket,
                            current: if action == "replace" { "X".into() } else { String::new() },
                        }])
                        .unwrap();
                }
                "discard" => {
                    window.resolve(WindowDecision::Discard).unwrap();
                }
                _ => {
                    window
                        .resolve(WindowDecision::Replace(Bytes::from_static(b"opaque")))
                        .unwrap();
                }
            }
            assert_eq!(window.resident().unwrap(), owned_capacity(&window));
            window.start_next().unwrap();
            window.retain(json_record(bucket, "next")).unwrap();
            let snapshot = window.snapshot_all().unwrap();
            assert_eq!(snapshot[0].context, expected, "{action}");
            assert_eq!(snapshot[0].current, "next");
        }
    }

    #[test]
    fn semantic_replacement_fairness_spans_prior_and_new_utf8_text() {
        let a = key(0, LogicalField::Content);
        let b = key(1, LogicalField::Content);
        let mut window = RawWindow::with_policy(RawWindowPolicy {
            per_bucket_overlap_bytes: 8,
            global_overlap_bytes: 8,
            max_replacement_bytes: MAX_REPLACEMENT_BYTES,
        })
        .unwrap();
        window.retain(json_record(a, "abcé")).unwrap();
        window.retain(json_record(b, "12345")).unwrap();
        window.resolve(WindowDecision::Release).unwrap();
        window.start_next().unwrap();
        window.retain(json_record(a, "hidden")).unwrap();
        window.retain(json_record(b, "hidden")).unwrap();
        window
            .resolve_semantic_replace(vec![
                SemanticReplacement {
                    key: b,
                    current: "Y".into(),
                },
                SemanticReplacement {
                    key: a,
                    current: "X".into(),
                },
            ])
            .unwrap();
        window.start_next().unwrap();
        let snapshot = window.snapshot_all().unwrap();
        assert_eq!(snapshot[0].context, "céX");
        assert_eq!(snapshot[1].context, "345Y");
    }

    #[test]
    fn context_only_bucket_cannot_install_non_emitted_replacement() {
        let a = key(0, LogicalField::Content);
        let b = key(1, LogicalField::Content);
        let mut window = RawWindow::new();
        window.retain(json_record(a, "prior")).unwrap();
        window.resolve(WindowDecision::Release).unwrap();
        window.start_next().unwrap();
        window.retain(json_record(b, "current")).unwrap();
        assert_eq!(
            window.resolve_semantic_replace(vec![
                SemanticReplacement {
                    key: a,
                    current: "ghost".into()
                },
                SemanticReplacement {
                    key: b,
                    current: "new".into()
                },
            ]),
            Err(WindowError::InvalidReplacement)
        );
        assert_eq!(window.snapshot_all().unwrap()[1].current, "current");
        let output = window
            .resolve_semantic_replace(vec![
                SemanticReplacement {
                    key: a,
                    current: String::new(),
                },
                SemanticReplacement {
                    key: b,
                    current: "new".into(),
                },
            ])
            .unwrap();
        assert_eq!(output, WindowOutput::Replace(Bytes::from_static(b"\"new\"")));
        window.start_next().unwrap();
        assert_eq!(window.snapshot_all().unwrap()[0].context, "prior");
    }

    #[test]
    fn raw_only_release_resets_context_across_an_unobservable_span() {
        let bucket = key(0, LogicalField::Content);
        let mut window = RawWindow::new();
        window.retain(json_record(bucket, "prior")).unwrap();
        window.resolve(WindowDecision::Release).unwrap();
        window.start_next().unwrap();
        window
            .retain(WindowRecord::raw(vec![Bytes::from_static(b"unknown")]).semantic_unavailable())
            .unwrap();
        assert_eq!(released(window.resolve(WindowDecision::Release).unwrap()), b"unknown");
        window.start_next().unwrap();
        window.retain(json_record(bucket, "next")).unwrap();
        assert_eq!(window.snapshot_all().unwrap()[0].context, "");
    }

    #[test]
    fn terminal_replacement_charges_inputs_but_never_builds_overlap() {
        let bucket = key(0, LogicalField::Content);
        for (finish, below, succeeds) in [(true, false, true), (true, true, false), (false, false, false)] {
            let mut window = RawWindow::new();
            window.retain(json_record(bucket, "old")).unwrap();
            let mut text = String::with_capacity(128);
            text.push('z');
            let mut replacements = Vec::with_capacity(2);
            replacements.push(SemanticReplacement {
                key: bucket,
                current: text,
            });
            let input = replacements.capacity() * size_of::<SemanticReplacement>() + replacements[0].current.capacity();
            let peak = owned_capacity(&window) + input + size_of::<ReplacementEdit>() + 3;
            window.limits.combined = 0;
            assert_eq!(window.preview_semantic_replace([(bucket, "z")]).unwrap(), 3);
            window.limits.combined = peak - usize::from(below);
            let output = if finish {
                window.finish_semantic_replace(replacements)
            } else {
                window.resolve_semantic_replace(replacements)
            };
            if succeeds {
                assert_eq!(output.unwrap(), WindowOutput::Replace(Bytes::from_static(b"\"z\"")));
                assert_eq!(window.start_next(), Err(WindowError::Finished));
            } else {
                assert_eq!(output, Err(WindowError::Limit(WindowLimit::CombinedResidentBytes)));
                window.limits.combined = MAX_COMBINED_RESIDENT_BYTES;
                assert_eq!(released(window.finish(WindowDecision::Release).unwrap()), b"\"old\"");
            }
        }
    }

    #[test]
    fn json_token_visitor_matches_serde_with_exact_scratch_and_output_caps() {
        let tokens: &[&[u8]] = &[
            br#""""#,
            br#""a\b\f\n\r\t\"\\\/""#,
            br#""\u20ac\u00e9""#,
            br#""\uD834\uDD1E""#,
            b" \t\"value\"\r\n",
            br#""\uD834""#,
            br#""\uDD1E""#,
            br#""\uD834\u0061""#,
            br#""\uXY00""#,
            br#""\q""#,
            br#""ok" false"#,
            b"\"\xff\"",
            b"\"a\n\"",
            b"\"tail\\",
        ];
        let bucket = key(0, LogicalField::Content);
        for &token in tokens {
            let oracle = serde_json::from_slice::<String>(token);
            for below in [false, true] {
                let mut window = RawWindow::new();
                window
                    .retain(
                        WindowRecord::raw(vec![Bytes::copy_from_slice(token)]).with_fragments(vec![SemanticFragment {
                            bucket,
                            source: SemanticSource::JsonStringTokenRanges(vec![ByteRange::new(0, token.len())]),
                        }]),
                    )
                    .unwrap();
                window.limits.combined =
                    owned_capacity(&window) + token.len() + oracle.as_ref().map_or(0, |s| s.len()) - usize::from(below);
                let actual = window.snapshot(bucket).unwrap().materialize();
                if below {
                    assert_eq!(
                        actual,
                        Err(WindowError::Limit(WindowLimit::CombinedResidentBytes)),
                        "{token:?}"
                    );
                } else if let Ok(expected) = &oracle {
                    assert_eq!(actual.as_ref().unwrap(), expected, "{token:?}");
                } else {
                    assert_eq!(actual, Err(WindowError::InvalidJsonString), "{token:?}");
                }
            }
        }
    }

    proptest::proptest! {
        #[test]
        fn json_token_count_and_write_match_serialized_allowed_scalars(
            chars in proptest::collection::vec(proptest::sample::select(vec!['a', 'é', '€', 'π', '𝄞', '"', '\\', '\n', '\t', '\r', '\0']), 0..128)
        ) {
            let expected: String = chars.into_iter().collect();
            let token = serde_json::to_vec(&expected).unwrap();
            let mut counted = 0;
            visit_json_string(&token, &mut |part| { counted += part.len(); Ok(()) }).unwrap();
            let mut actual = String::with_capacity(counted);
            visit_json_string(&token, &mut |part| { actual.push_str(part); Ok(()) }).unwrap();
            proptest::prop_assert_eq!(counted, expected.len());
            proptest::prop_assert_eq!(actual, expected);
        }
    }

    #[test]
    fn snapshot_all_charges_all_strings_context_and_result_metadata() {
        let a = key(0, LogicalField::Content);
        let b = key(1, LogicalField::Content);
        for below in [false, true] {
            let mut window = RawWindow::new();
            window.retain(json_record(a, "x")).unwrap();
            window.retain(json_record(b, "y")).unwrap();
            window.resolve(WindowDecision::Release).unwrap();
            window.start_next().unwrap();
            window.retain(json_record(a, "a")).unwrap();
            window.retain(json_record(b, "b")).unwrap();
            window.limits.combined =
                owned_capacity(&window) + 3 + 2 * size_of::<SemanticWindow>() + 4 - usize::from(below);
            let result = window.snapshot_all();
            if below {
                assert_eq!(result, Err(WindowError::Limit(WindowLimit::CombinedResidentBytes)));
            } else {
                let snapshot = result.unwrap();
                assert_eq!((&*snapshot[0].context, &*snapshot[0].current), ("x", "a"));
                assert_eq!((&*snapshot[1].context, &*snapshot[1].current), ("y", "b"));
            }
        }
    }

    #[test]
    fn reused_raw_table_is_charged_once_and_rejection_preserves_input() {
        let mut window = RawWindow::new();
        window
            .retain(WindowRecord::raw(vec![Bytes::from_static(b"a")]))
            .unwrap();
        let record = WindowRecord::raw(vec![Bytes::from_static(b"b")]);
        let pointer = record.slices.as_ptr();
        let entry_size = window.raw.slice_metadata_capacity() / MAX_SLICE_HANDLES;
        let peak = owned_capacity(&window) + record.metadata_bytes() + entry_size + 1;
        window.limits.combined = peak - 1;
        let (error, record) = window.try_retain(record).unwrap_err();
        assert_eq!(error, WindowError::Limit(WindowLimit::CombinedResidentBytes));
        assert_eq!(record.slices.as_ptr(), pointer);
        assert_eq!(window.raw.len(), 1);
        window.limits.combined = peak;
        window.try_retain(record).unwrap();
        assert_eq!(released(window.finish(WindowDecision::Release).unwrap()), b"ab");
    }

    #[test]
    fn coalescing_and_arena_append_admit_before_consuming_the_record() {
        let mut window = RawWindow::new();
        for _ in 0..MAX_SLICE_HANDLES {
            window
                .retain(WindowRecord::raw(vec![Bytes::from_static(b"a")]))
                .unwrap();
        }
        for (text, additional) in [(b"y", MAX_RAW_BYTES), (b"z", 0)] {
            let record = WindowRecord::raw(vec![Bytes::from_static(text)]);
            let pointer = record.slices.as_ptr();
            let peak = owned_capacity(&window) + record.metadata_bytes() + additional;
            window.limits.combined = peak - 1;
            let (error, record) = window.try_retain(record).unwrap_err();
            assert_eq!(error, WindowError::Limit(WindowLimit::CombinedResidentBytes));
            assert_eq!(record.slices.as_ptr(), pointer);
            window.limits.combined = peak;
            window.try_retain(record).unwrap();
        }
        assert_eq!(window.raw.coalesce_count(), 1);
        let mut expected = vec![b'a'; MAX_SLICE_HANDLES];
        expected.extend_from_slice(b"yz");
        assert_eq!(released(window.finish(WindowDecision::Release).unwrap()), expected);
    }

    #[test]
    fn repeated_bucket_append_moves_ranges_without_cloning_allocations() {
        let bucket = key(0, LogicalField::Content);
        let mut window = RawWindow::new();
        let mut pointers = Vec::new();
        let mut capacity_changes = 0;
        let mut previous_capacity = 0;
        for index in 0..130 {
            let mut record = json_record(bucket, "v");
            if index == 2 {
                let original_ranges = record.fragments[0].source.ranges().as_ptr();
                let before = window.accounting();
                window.limits.combined = 0;
                let (error, returned) = window.try_retain(record).unwrap_err();
                assert_eq!(error, WindowError::Limit(WindowLimit::CombinedResidentBytes));
                assert_eq!(returned.fragments[0].source.ranges().as_ptr(), original_ranges);
                assert_eq!(returned.fragments[0].source.ranges(), &[ByteRange::new(0, 3)]);
                assert_eq!(window.accounting(), before);
                window.limits.combined = MAX_COMBINED_RESIDENT_BYTES;
                record = returned;
            }
            pointers.push(record.fragments[0].source.ranges().as_ptr());
            window.retain(record).unwrap();
            assert_eq!(window.resident().unwrap(), owned_capacity(&window));
            let fragments = &window.buckets[0].fragments;
            for (fragment, &pointer) in fragments.iter().zip(&pointers) {
                assert_eq!(fragment.source.ranges().as_ptr(), pointer);
            }
            capacity_changes += usize::from(fragments.capacity() != previous_capacity);
            previous_capacity = fragments.capacity();
        }
        assert_eq!(capacity_changes, 9);
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
