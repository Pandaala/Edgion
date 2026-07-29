use async_trait::async_trait;
use bytes::{Bytes, BytesMut};
use parking_lot::RwLock;
use pingora_core::protocols::http::RequestBodyBuffer;
use pingora_core::{Error as PingoraError, ErrorType, Result as PingoraResult};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io;
use std::ops::Range;
use std::os::unix::fs::FileExt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use tempfile::NamedTempFile;
use tokio::sync::{OnceCell, Semaphore};

use super::operation::{
    BodyOperationError, BodyScanResult, BodyTextOperation, BodyTextResult, BytesScanner, LossyTextScanner,
    RawBodyScanner, ScanControl, TextOperationScanner, MAX_BODY_SEGMENTS,
};
use crate::plugins::runtime::traits::session::{BodyReadError, BodyReadResult, RequestBodySnapshotReader};

const SCAN_CHUNK_SIZE: usize = 64 * 1024;
const DISK_SCAN_CONCURRENCY: usize = 32;

fn disk_scan_limiter() -> &'static Arc<Semaphore> {
    static LIMITER: OnceLock<Arc<Semaphore>> = OnceLock::new();
    LIMITER.get_or_init(|| Arc::new(Semaphore::new(DISK_SCAN_CONCURRENCY)))
}

#[derive(Clone)]
enum BodySource {
    Memory(Bytes),
    Disk { file: Arc<File>, len: u64 },
}

#[derive(Clone)]
enum Segment {
    SourceRange {
        source: Arc<BodySource>,
        offset: u64,
        len: u64,
    },
    Inline(Bytes),
}

impl Segment {
    fn len(&self) -> Result<u64, BodyOperationError> {
        match self {
            Self::SourceRange { len, .. } => Ok(*len),
            Self::Inline(bytes) => {
                u64::try_from(bytes.len()).map_err(|_| BodyOperationError::Body(BodyReadError::Incomplete))
            }
        }
    }
}

#[derive(Clone)]
struct BodySnapshot {
    segments: Vec<Segment>,
    len: u64,
}

impl BodySnapshot {
    fn empty() -> Self {
        Self {
            segments: Vec::new(),
            len: 0,
        }
    }

    fn from_source(source: Arc<BodySource>) -> Result<Self, BodyReadError> {
        let len = match source.as_ref() {
            BodySource::Memory(bytes) => u64::try_from(bytes.len()).map_err(|_| BodyReadError::Incomplete)?,
            BodySource::Disk { len, .. } => *len,
        };
        if len == 0 {
            return Ok(Self::empty());
        }
        Ok(Self {
            segments: vec![Segment::SourceRange { source, offset: 0, len }],
            len,
        })
    }

    fn from_inline(body: Bytes) -> Result<Self, BodyOperationError> {
        if body.is_empty() {
            return Ok(Self::empty());
        }
        let len = u64::try_from(body.len()).map_err(|_| BodyOperationError::Body(BodyReadError::Incomplete))?;
        Ok(Self {
            segments: vec![Segment::Inline(body)],
            len,
        })
    }

    fn from_segments(segments: Vec<Segment>) -> Result<Self, BodyOperationError> {
        let segments = normalize_segments(segments)?;
        if segments.len() > MAX_BODY_SEGMENTS {
            return Err(BodyOperationError::SegmentLimitExceeded {
                limit: MAX_BODY_SEGMENTS,
            });
        }
        let len = segments.iter().try_fold(0u64, |total, segment| {
            total
                .checked_add(segment.len()?)
                .ok_or(BodyOperationError::Body(BodyReadError::Incomplete))
        })?;
        Ok(Self { segments, len })
    }

    fn contains_disk(&self) -> bool {
        self.segments.iter().any(|segment| {
            matches!(
                segment,
                Segment::SourceRange {
                    source,
                    ..
                } if matches!(source.as_ref(), BodySource::Disk { .. })
            )
        })
    }

    fn slice(&self, range: Range<u64>) -> Result<Vec<Segment>, BodyOperationError> {
        if range.start > range.end || range.end > self.len {
            return Err(BodyOperationError::InvalidRange);
        }
        if range.is_empty() {
            return Ok(Vec::new());
        }

        let mut logical_offset = 0u64;
        let mut output = Vec::new();
        for segment in &self.segments {
            let segment_len = segment.len()?;
            let segment_end = logical_offset
                .checked_add(segment_len)
                .ok_or(BodyOperationError::Body(BodyReadError::Incomplete))?;
            let start = range.start.max(logical_offset);
            let end = range.end.min(segment_end);
            if start < end {
                let local_start = start - logical_offset;
                let local_len = end - start;
                output.push(slice_segment(segment, local_start, local_len)?);
            }
            logical_offset = segment_end;
            if logical_offset >= range.end {
                break;
            }
        }
        Ok(output)
    }
}

fn slice_segment(segment: &Segment, local_start: u64, len: u64) -> Result<Segment, BodyOperationError> {
    match segment {
        Segment::SourceRange {
            source,
            offset,
            len: segment_len,
        } => {
            let end = local_start
                .checked_add(len)
                .ok_or(BodyOperationError::Body(BodyReadError::Incomplete))?;
            if end > *segment_len {
                return Err(BodyOperationError::InvalidRange);
            }
            Ok(Segment::SourceRange {
                source: Arc::clone(source),
                offset: offset
                    .checked_add(local_start)
                    .ok_or(BodyOperationError::Body(BodyReadError::Incomplete))?,
                len,
            })
        }
        Segment::Inline(bytes) => {
            let start = usize::try_from(local_start).map_err(|_| BodyOperationError::InvalidRange)?;
            let len = usize::try_from(len).map_err(|_| BodyOperationError::InvalidRange)?;
            let end = start.checked_add(len).ok_or(BodyOperationError::InvalidRange)?;
            if end > bytes.len() {
                return Err(BodyOperationError::InvalidRange);
            }
            Ok(Segment::Inline(bytes.slice(start..end)))
        }
    }
}

fn normalize_segments(segments: Vec<Segment>) -> Result<Vec<Segment>, BodyOperationError> {
    let mut normalized: Vec<Segment> = Vec::with_capacity(segments.len());
    for segment in segments {
        if segment.len()? == 0 {
            continue;
        }
        if let (
            Some(Segment::SourceRange {
                source: previous_source,
                offset: previous_offset,
                len: previous_len,
            }),
            Segment::SourceRange { source, offset, len },
        ) = (normalized.last_mut(), &segment)
        {
            let previous_end = previous_offset
                .checked_add(*previous_len)
                .ok_or(BodyOperationError::Body(BodyReadError::Incomplete))?;
            if Arc::ptr_eq(previous_source, source) && previous_end == *offset {
                *previous_len = previous_len
                    .checked_add(*len)
                    .ok_or(BodyOperationError::Body(BodyReadError::Incomplete))?;
                continue;
            }
        }
        normalized.push(segment);
        if normalized.len() > MAX_BODY_SEGMENTS {
            return Err(BodyOperationError::SegmentLimitExceeded {
                limit: MAX_BODY_SEGMENTS,
            });
        }
    }
    Ok(normalized)
}

#[derive(Default)]
struct MutationState {
    snapshot: Option<Arc<BodySnapshot>>,
    generation: u64,
}

struct BodyStorageViewInner {
    source: OnceLock<Arc<BodySource>>,
    materialized: OnceCell<Bytes>,
    mutation: RwLock<MutationState>,
    max_body_size_bytes: u64,
}

/// Shared request-body handle captured by request-stage plugins.
///
/// Every read takes an immutable logical snapshot. A mutation publishes a new
/// snapshot only after all bounds and scans succeed; existing replay attempts
/// continue using the snapshot captured by their last rewind.
pub struct BodyStorageView {
    inner: Arc<BodyStorageViewInner>,
}

impl BodyStorageView {
    fn new(inner: Arc<BodyStorageViewInner>) -> Self {
        Self { inner }
    }

    fn snapshot(&self) -> Result<(Arc<BodySnapshot>, u64), BodyReadError> {
        let mutation = self.inner.mutation.read();
        if let Some(snapshot) = mutation.snapshot.as_ref() {
            return Ok((Arc::clone(snapshot), mutation.generation));
        }
        let source = Arc::clone(self.inner.source.get().ok_or(BodyReadError::Incomplete)?);
        Ok((Arc::new(BodySnapshot::from_source(source)?), mutation.generation))
    }

    fn publish(&self, expected_generation: u64, snapshot: BodySnapshot) -> Result<(), BodyOperationError> {
        if snapshot.len > self.inner.max_body_size_bytes {
            return Err(BodyOperationError::Body(BodyReadError::BufferLimitExceeded {
                limit: self.inner.max_body_size_bytes,
            }));
        }
        let snapshot = Arc::new(snapshot);
        let mut mutation = self.inner.mutation.write();
        if mutation.generation != expected_generation {
            return Err(BodyOperationError::Body(BodyReadError::Incomplete));
        }
        mutation.generation = mutation
            .generation
            .checked_add(1)
            .ok_or(BodyOperationError::Body(BodyReadError::Incomplete))?;
        mutation.snapshot = Some(snapshot);
        Ok(())
    }

    pub async fn read_all(&self) -> BodyReadResult {
        let mutation = self.inner.mutation.read().snapshot.clone();
        if let Some(snapshot) = mutation {
            let capacity = usize::try_from(snapshot.len).map_err(|_| BodyReadError::Incomplete)?;
            return self
                .scan_snapshot(snapshot, BytesScanner::with_capacity(capacity), u64::MAX)
                .await
                .map(|result| result.value)
                .map_err(operation_to_body_error);
        }

        let source = self.inner.source.get().ok_or(BodyReadError::Incomplete)?;
        match source.as_ref() {
            BodySource::Memory(body) => Ok(body.clone()),
            BodySource::Disk { file, len } => {
                let body = self
                    .inner
                    .materialized
                    .get_or_try_init(|| read_disk_body(Arc::clone(file), *len))
                    .await?;
                Ok(body.clone())
            }
        }
    }

    pub fn len(&self) -> Result<u64, BodyReadError> {
        self.snapshot().map(|(snapshot, _)| snapshot.len)
    }

    /// Capture an owned reader over the current immutable logical snapshot.
    ///
    /// Creating the reader does not materialize a disk-backed source. Later
    /// mutations publish a different snapshot and cannot affect this reader.
    pub fn snapshot_reader(&self) -> Result<Box<dyn RequestBodySnapshotReader>, BodyReadError> {
        let (snapshot, _) = self.snapshot()?;
        Ok(Box::new(BodySnapshotReader::new(snapshot)))
    }

    pub fn is_modified(&self) -> bool {
        self.inner.mutation.read().snapshot.is_some()
    }

    #[cfg(test)]
    pub(crate) fn is_materialized(&self) -> bool {
        self.inner.materialized.get().is_some()
    }

    pub async fn scan_raw<S>(
        &self,
        scanner: S,
        max_scan_bytes: u64,
    ) -> Result<BodyScanResult<S::Output>, BodyOperationError>
    where
        S: RawBodyScanner,
    {
        let (snapshot, _) = self.snapshot().map_err(BodyOperationError::Body)?;
        self.scan_snapshot(snapshot, scanner, max_scan_bytes).await
    }

    async fn scan_snapshot<S>(
        &self,
        snapshot: Arc<BodySnapshot>,
        scanner: S,
        max_scan_bytes: u64,
    ) -> Result<BodyScanResult<S::Output>, BodyOperationError>
    where
        S: RawBodyScanner,
    {
        if !scanner.requires_blocking_context() && !snapshot.contains_disk() && snapshot.len <= SCAN_CHUNK_SIZE as u64 {
            return scan_snapshot_owned(snapshot, scanner, max_scan_bytes, None);
        }

        let permit = if snapshot.contains_disk() {
            Some(
                Arc::clone(disk_scan_limiter())
                    .acquire_owned()
                    .await
                    .map_err(|_| BodyOperationError::Body(BodyReadError::Incomplete))?,
            )
        } else {
            None
        };
        let cancelled = Arc::new(AtomicBool::new(false));
        let task_cancelled = Arc::clone(&cancelled);
        let mut guard = CancellationGuard {
            cancelled,
            completed: false,
        };
        let result = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            scan_snapshot_owned(snapshot, scanner, max_scan_bytes, Some(&task_cancelled))
        })
        .await
        .map_err(|_| BodyOperationError::Body(BodyReadError::Incomplete))?;
        guard.completed = true;
        result
    }

    pub async fn text_operation(
        &self,
        operation: BodyTextOperation,
        max_scan_bytes: u64,
    ) -> Result<BodyScanResult<BodyTextResult>, BodyOperationError> {
        let len = self.len().map_err(BodyOperationError::Body)?;
        let fast_result = match &operation {
            BodyTextOperation::IsEmpty => Some(len == 0),
            BodyTextOperation::StartsWith(pattern)
            | BodyTextOperation::EndsWith(pattern)
            | BodyTextOperation::Contains(pattern)
                if pattern.is_empty() =>
            {
                Some(true)
            }
            BodyTextOperation::StartsWith(_)
            | BodyTextOperation::EndsWith(_)
            | BodyTextOperation::Contains(_)
            | BodyTextOperation::Equals(_)
                if len == 0 =>
            {
                Some(matches!(&operation, BodyTextOperation::Equals(pattern) if pattern.is_empty()))
            }
            BodyTextOperation::Equals(pattern) if pattern.is_empty() => Some(false),
            _ => None,
        };
        if let Some(value) = fast_result {
            return Ok(BodyScanResult {
                value: BodyTextResult::Bool(value),
                bytes_scanned: 0,
            });
        }
        self.scan_raw(
            LossyTextScanner::new(TextOperationScanner::new(operation)),
            max_scan_bytes,
        )
        .await
    }

    pub async fn read_range(
        &self,
        offset: u64,
        max_bytes: u64,
        max_scan_bytes: u64,
    ) -> Result<BodyScanResult<Bytes>, BodyOperationError> {
        let (snapshot, _) = self.snapshot().map_err(BodyOperationError::Body)?;
        if offset > snapshot.len {
            return Err(BodyOperationError::InvalidRange);
        }
        let end = offset.checked_add(max_bytes).unwrap_or(u64::MAX).min(snapshot.len);
        let slice = Arc::new(BodySnapshot::from_segments(snapshot.slice(offset..end)?)?);
        let capacity = usize::try_from(slice.len).map_err(|_| BodyOperationError::Body(BodyReadError::Incomplete))?;
        self.scan_snapshot(slice, BytesScanner::with_capacity(capacity), max_scan_bytes)
            .await
    }

    pub async fn sha256_scan(&self, max_scan_bytes: u64) -> Result<BodyScanResult<[u8; 32]>, BodyOperationError> {
        self.scan_raw(Sha256Scanner(Sha256::new()), max_scan_bytes).await
    }

    pub fn clear(&self) -> Result<(), BodyOperationError> {
        let (_, generation) = self.snapshot().map_err(BodyOperationError::Body)?;
        self.publish(generation, BodySnapshot::empty())
    }

    pub fn append(&self, bytes: Bytes) -> Result<(), BodyOperationError> {
        if bytes.is_empty() {
            return Ok(());
        }
        let (snapshot, generation) = self.snapshot().map_err(BodyOperationError::Body)?;
        let mut segments = snapshot.segments.clone();
        segments.push(Segment::Inline(bytes));
        self.publish(generation, BodySnapshot::from_segments(segments)?)
    }

    pub fn prepend(&self, bytes: Bytes) -> Result<(), BodyOperationError> {
        if bytes.is_empty() {
            return Ok(());
        }
        let (snapshot, generation) = self.snapshot().map_err(BodyOperationError::Body)?;
        let mut segments = Vec::with_capacity(snapshot.segments.len() + 1);
        segments.push(Segment::Inline(bytes));
        segments.extend(snapshot.segments.iter().cloned());
        self.publish(generation, BodySnapshot::from_segments(segments)?)
    }

    pub fn remove(&self, range: Range<u64>) -> Result<(), BodyOperationError> {
        let (snapshot, generation) = self.snapshot().map_err(BodyOperationError::Body)?;
        if range.start > range.end || range.end > snapshot.len {
            return Err(BodyOperationError::InvalidRange);
        }
        let mut segments = snapshot.slice(0..range.start)?;
        segments.extend(snapshot.slice(range.end..snapshot.len)?);
        self.publish(generation, BodySnapshot::from_segments(segments)?)
    }

    pub async fn replace_literal(
        &self,
        from: Bytes,
        to: Bytes,
        max_scan_bytes: u64,
    ) -> Result<BodyScanResult<()>, BodyOperationError> {
        if from.is_empty() {
            return Err(BodyOperationError::EmptyPattern);
        }
        let (snapshot, generation) = self.snapshot().map_err(BodyOperationError::Body)?;
        let scan = self
            .scan_snapshot(
                Arc::clone(&snapshot),
                LiteralMatchScanner::new(from.to_vec()),
                max_scan_bytes,
            )
            .await?;
        if scan.value.is_empty() {
            return Ok(BodyScanResult {
                value: (),
                bytes_scanned: scan.bytes_scanned,
            });
        }

        let match_count =
            u64::try_from(scan.value.len()).map_err(|_| BodyOperationError::Body(BodyReadError::Incomplete))?;
        let from_len = u64::try_from(from.len()).map_err(|_| BodyOperationError::Body(BodyReadError::Incomplete))?;
        let to_len = u64::try_from(to.len()).map_err(|_| BodyOperationError::Body(BodyReadError::Incomplete))?;
        let removed = match_count
            .checked_mul(from_len)
            .ok_or(BodyOperationError::Body(BodyReadError::Incomplete))?;
        let added = match_count
            .checked_mul(to_len)
            .ok_or(BodyOperationError::Body(BodyReadError::Incomplete))?;
        let output_len = snapshot
            .len
            .checked_sub(removed)
            .and_then(|len| len.checked_add(added))
            .ok_or(BodyOperationError::Body(BodyReadError::Incomplete))?;
        if output_len > self.inner.max_body_size_bytes {
            return Err(BodyOperationError::Body(BodyReadError::BufferLimitExceeded {
                limit: self.inner.max_body_size_bytes,
            }));
        }

        let mut segments = Vec::new();
        let mut cursor = 0u64;
        for range in scan.value {
            segments.extend(snapshot.slice(cursor..range.start)?);
            if !to.is_empty() {
                segments.push(Segment::Inline(to.clone()));
            }
            cursor = range.end;
            if segments.len() > MAX_BODY_SEGMENTS {
                return Err(BodyOperationError::SegmentLimitExceeded {
                    limit: MAX_BODY_SEGMENTS,
                });
            }
        }
        segments.extend(snapshot.slice(cursor..snapshot.len)?);
        let replacement = BodySnapshot::from_segments(segments)?;
        debug_assert_eq!(replacement.len, output_len);
        self.publish(generation, replacement)?;
        Ok(BodyScanResult {
            value: (),
            bytes_scanned: scan.bytes_scanned,
        })
    }

    /// Compatibility wrapper for existing body-mutating plugins. New callers
    /// should use range mutation plus an explicit read only when bytes are
    /// genuinely required.
    pub async fn replace_slice(&self, range: Range<usize>, replacement: Bytes) -> BodyReadResult {
        let start = u64::try_from(range.start).map_err(|_| BodyReadError::Incomplete)?;
        let end = u64::try_from(range.end).map_err(|_| BodyReadError::Incomplete)?;
        let (snapshot, generation) = self.snapshot()?;
        if start > end || end > snapshot.len {
            return Err(BodyReadError::Incomplete);
        }
        let mut segments = snapshot.slice(0..start).map_err(operation_to_body_error)?;
        if !replacement.is_empty() {
            segments.push(Segment::Inline(replacement));
        }
        segments.extend(snapshot.slice(end..snapshot.len).map_err(operation_to_body_error)?);
        let next = BodySnapshot::from_segments(segments).map_err(operation_to_body_error)?;
        if next.len > self.inner.max_body_size_bytes {
            return Err(BodyReadError::BufferLimitExceeded {
                limit: self.inner.max_body_size_bytes,
            });
        }
        let capacity = usize::try_from(next.len).map_err(|_| BodyReadError::Incomplete)?;
        let result = self
            .scan_snapshot(Arc::new(next.clone()), BytesScanner::with_capacity(capacity), u64::MAX)
            .await
            .map_err(operation_to_body_error)?
            .value;
        self.publish(generation, next).map_err(operation_to_body_error)?;
        Ok(result)
    }

    pub fn overwrite_body(&self, body: Bytes) -> Result<(), BodyReadError> {
        let (_, generation) = self.snapshot()?;
        let snapshot = BodySnapshot::from_inline(body).map_err(operation_to_body_error)?;
        self.publish(generation, snapshot).map_err(operation_to_body_error)
    }

    pub async fn sha256(&self) -> Result<[u8; 32], BodyReadError> {
        self.sha256_scan(u64::MAX)
            .await
            .map(|result| result.value)
            .map_err(operation_to_body_error)
    }
}

struct BodySnapshotReader {
    snapshot: Arc<BodySnapshot>,
    segment_index: usize,
    segment_offset: u64,
}

impl BodySnapshotReader {
    fn new(snapshot: Arc<BodySnapshot>) -> Self {
        Self {
            snapshot,
            segment_index: 0,
            segment_offset: 0,
        }
    }
}

#[async_trait]
impl RequestBodySnapshotReader for BodySnapshotReader {
    fn len(&self) -> u64 {
        self.snapshot.len
    }

    async fn next_chunk(&mut self) -> Result<Option<Bytes>, BodyReadError> {
        loop {
            let Some(segment) = self.snapshot.segments.get(self.segment_index) else {
                return Ok(None);
            };
            let segment_len = segment.len().map_err(operation_to_body_error)?;
            if self.segment_offset == segment_len {
                self.segment_index += 1;
                self.segment_offset = 0;
                continue;
            }

            let wanted = usize::try_from((segment_len - self.segment_offset).min(SCAN_CHUNK_SIZE as u64))
                .map_err(|_| BodyReadError::Incomplete)?;
            let chunk = match segment {
                Segment::SourceRange { source, .. } if matches!(source.as_ref(), BodySource::Disk { .. }) => {
                    let permit = Arc::clone(disk_scan_limiter())
                        .acquire_owned()
                        .await
                        .map_err(|_| BodyReadError::Incomplete)?;
                    let segment = segment.clone();
                    let segment_offset = self.segment_offset;
                    tokio::task::spawn_blocking(move || {
                        let _permit = permit;
                        read_segment_chunk(&segment, segment_offset, wanted)
                    })
                    .await
                    .map_err(|_| BodyReadError::Incomplete)?
                    .map_err(operation_to_body_error)?
                }
                _ => read_segment_chunk(segment, self.segment_offset, wanted).map_err(operation_to_body_error)?,
            };
            if chunk.is_empty() {
                return Err(BodyReadError::Incomplete);
            }
            let chunk_len = u64::try_from(chunk.len()).map_err(|_| BodyReadError::Incomplete)?;
            self.segment_offset = self
                .segment_offset
                .checked_add(chunk_len)
                .ok_or(BodyReadError::Incomplete)?;
            return Ok(Some(chunk));
        }
    }
}

fn operation_to_body_error(error: BodyOperationError) -> BodyReadError {
    match error {
        BodyOperationError::Body(error) => error,
        BodyOperationError::ScanLimitExceeded { limit } => BodyReadError::TooLarge { limit },
        BodyOperationError::SegmentLimitExceeded { .. }
        | BodyOperationError::InvalidRange
        | BodyOperationError::EmptyPattern => BodyReadError::Incomplete,
    }
}

struct CancellationGuard {
    cancelled: Arc<AtomicBool>,
    completed: bool,
}

impl Drop for CancellationGuard {
    fn drop(&mut self) {
        if !self.completed {
            self.cancelled.store(true, Ordering::Release);
        }
    }
}

fn scan_snapshot_owned<S>(
    snapshot: Arc<BodySnapshot>,
    mut scanner: S,
    max_scan_bytes: u64,
    cancelled: Option<&AtomicBool>,
) -> Result<BodyScanResult<S::Output>, BodyOperationError>
where
    S: RawBodyScanner,
{
    let mut bytes_scanned = 0u64;
    for segment in &snapshot.segments {
        let segment_len = segment.len()?;
        let mut local_offset = 0u64;
        while local_offset < segment_len {
            if cancelled.is_some_and(|flag| flag.load(Ordering::Acquire)) {
                return Err(BodyOperationError::Body(BodyReadError::Incomplete));
            }
            if bytes_scanned == max_scan_bytes {
                return Err(BodyOperationError::ScanLimitExceeded { limit: max_scan_bytes });
            }
            let remaining_budget = max_scan_bytes - bytes_scanned;
            let wanted = usize::try_from(
                (segment_len - local_offset)
                    .min(SCAN_CHUNK_SIZE as u64)
                    .min(remaining_budget),
            )
            .map_err(|_| BodyOperationError::Body(BodyReadError::Incomplete))?;
            let chunk = read_segment_chunk(segment, local_offset, wanted)?;
            if chunk.is_empty() {
                return Err(BodyOperationError::Body(BodyReadError::Incomplete));
            }
            let chunk_len =
                u64::try_from(chunk.len()).map_err(|_| BodyOperationError::Body(BodyReadError::Incomplete))?;
            bytes_scanned = bytes_scanned
                .checked_add(chunk_len)
                .ok_or(BodyOperationError::Body(BodyReadError::Incomplete))?;
            local_offset = local_offset
                .checked_add(chunk_len)
                .ok_or(BodyOperationError::Body(BodyReadError::Incomplete))?;
            if scanner.push(&chunk)? == ScanControl::Stop {
                return Ok(BodyScanResult {
                    value: scanner.finish()?,
                    bytes_scanned,
                });
            }
        }
    }
    Ok(BodyScanResult {
        value: scanner.finish()?,
        bytes_scanned,
    })
}

fn read_segment_chunk(segment: &Segment, local_offset: u64, wanted: usize) -> Result<Bytes, BodyOperationError> {
    match segment {
        Segment::Inline(bytes) => {
            let start =
                usize::try_from(local_offset).map_err(|_| BodyOperationError::Body(BodyReadError::Incomplete))?;
            Ok(bytes.slice(start..start + wanted))
        }
        Segment::SourceRange { source, offset, len } => {
            let end = local_offset
                .checked_add(wanted as u64)
                .ok_or(BodyOperationError::Body(BodyReadError::Incomplete))?;
            if end > *len {
                return Err(BodyOperationError::Body(BodyReadError::Incomplete));
            }
            let source_offset = offset
                .checked_add(local_offset)
                .ok_or(BodyOperationError::Body(BodyReadError::Incomplete))?;
            match source.as_ref() {
                BodySource::Memory(bytes) => {
                    let start = usize::try_from(source_offset)
                        .map_err(|_| BodyOperationError::Body(BodyReadError::Incomplete))?;
                    Ok(bytes.slice(start..start + wanted))
                }
                BodySource::Disk { file, len: source_len } => {
                    if source_offset
                        .checked_add(wanted as u64)
                        .is_none_or(|end| end > *source_len)
                    {
                        return Err(BodyOperationError::Body(BodyReadError::Incomplete));
                    }
                    let bytes = read_at_most(file, wanted, source_offset)
                        .map_err(|_| BodyOperationError::Body(BodyReadError::Incomplete))?;
                    if bytes.len() != wanted {
                        return Err(BodyOperationError::Body(BodyReadError::Incomplete));
                    }
                    Ok(Bytes::from(bytes))
                }
            }
        }
    }
}

struct Sha256Scanner(Sha256);

impl RawBodyScanner for Sha256Scanner {
    type Output = [u8; 32];

    fn push(&mut self, chunk: &[u8]) -> Result<ScanControl, BodyOperationError> {
        self.0.update(chunk);
        Ok(ScanControl::Continue)
    }

    fn finish(self) -> Result<Self::Output, BodyOperationError> {
        Ok(self.0.finalize().into())
    }
}

struct LiteralMatchScanner {
    pattern: Vec<u8>,
    failure: Vec<usize>,
    matched: usize,
    offset: u64,
    matches: Vec<Range<u64>>,
}

impl LiteralMatchScanner {
    fn new(pattern: Vec<u8>) -> Self {
        let mut failure = vec![0; pattern.len()];
        let mut matched = 0usize;
        for index in 1..pattern.len() {
            while matched > 0 && pattern[index] != pattern[matched] {
                matched = failure[matched - 1];
            }
            if pattern[index] == pattern[matched] {
                matched += 1;
                failure[index] = matched;
            }
        }
        Self {
            pattern,
            failure,
            matched: 0,
            offset: 0,
            matches: Vec::new(),
        }
    }
}

impl RawBodyScanner for LiteralMatchScanner {
    type Output = Vec<Range<u64>>;

    fn push(&mut self, chunk: &[u8]) -> Result<ScanControl, BodyOperationError> {
        for value in chunk {
            while self.matched > 0 && self.pattern[self.matched] != *value {
                self.matched = self.failure[self.matched - 1];
            }
            if self.pattern[self.matched] == *value {
                self.matched += 1;
            }
            self.offset = self
                .offset
                .checked_add(1)
                .ok_or(BodyOperationError::Body(BodyReadError::Incomplete))?;
            if self.matched == self.pattern.len() {
                let pattern_len = u64::try_from(self.pattern.len())
                    .map_err(|_| BodyOperationError::Body(BodyReadError::Incomplete))?;
                self.matches.push((self.offset - pattern_len)..self.offset);
                if self.matches.len() > MAX_BODY_SEGMENTS {
                    return Err(BodyOperationError::SegmentLimitExceeded {
                        limit: MAX_BODY_SEGMENTS,
                    });
                }
                self.matched = 0;
            }
        }
        Ok(ScanControl::Continue)
    }

    fn finish(self) -> Result<Self::Output, BodyOperationError> {
        Ok(self.matches)
    }
}

enum CaptureStorage {
    Memory(BytesMut),
    Disk { temp_file: NamedTempFile, file: Arc<File> },
    FinishedMemory(Bytes),
    FinishedDisk { temp_file: NamedTempFile, file: Arc<File> },
    Poisoned,
}

/// Request-body capture and replay storage supplied to Pingora's buffering seam.
pub struct BodyStorage {
    storage: CaptureStorage,
    view: Arc<BodyStorageViewInner>,
    memory_limit: usize,
    captured_len: u64,
    replay_cursor: u64,
    replay_invalid: bool,
    replay_snapshot: Option<Arc<BodySnapshot>>,
    finished: bool,
}

impl BodyStorage {
    pub fn new(memory_limit: usize, max_body_size_bytes: u64) -> (Self, Arc<BodyStorageView>) {
        Self::with_limits(memory_limit, max_body_size_bytes)
    }

    #[cfg(test)]
    fn with_memory_limit(memory_limit: usize) -> (Self, Arc<BodyStorageView>) {
        Self::with_limits(memory_limit, u64::MAX)
    }

    fn with_limits(memory_limit: usize, max_body_size_bytes: u64) -> (Self, Arc<BodyStorageView>) {
        let view = Arc::new(BodyStorageViewInner {
            source: OnceLock::new(),
            materialized: OnceCell::new(),
            mutation: RwLock::new(MutationState::default()),
            max_body_size_bytes,
        });
        (
            Self {
                storage: CaptureStorage::Memory(BytesMut::new()),
                view: Arc::clone(&view),
                memory_limit,
                captured_len: 0,
                replay_cursor: 0,
                replay_invalid: false,
                replay_snapshot: None,
                finished: false,
            },
            Arc::new(BodyStorageView::new(view)),
        )
    }

    fn publish(&self, source: BodySource) -> PingoraResult<()> {
        self.view.source.set(Arc::new(source)).map_err(|_| {
            PingoraError::explain(
                ErrorType::InternalError,
                "request body storage was finalized more than once with different state",
            )
        })
    }

    fn replay_error(message: impl Into<String>) -> Box<PingoraError> {
        PingoraError::explain(ErrorType::InternalError, message.into())
    }
}

#[async_trait]
impl RequestBodyBuffer for BodyStorage {
    async fn write(&mut self, data: &Bytes) -> PingoraResult<()> {
        if self.finished {
            return Err(Self::replay_error("request body capture continued after finalization"));
        }
        let data_len = u64::try_from(data.len())
            .map_err(|_| Self::replay_error("request body chunk length does not fit in u64"))?;
        let next_len = self
            .captured_len
            .checked_add(data_len)
            .ok_or_else(|| Self::replay_error("request body captured length overflow"))?;
        if next_len > self.view.max_body_size_bytes {
            return Err(PingoraError::explain(
                ErrorType::HTTPStatus(413),
                "request body capture exceeds physical limit",
            ));
        }

        match &mut self.storage {
            CaptureStorage::Memory(buf) if next_len <= self.memory_limit as u64 => {
                buf.extend_from_slice(data);
            }
            CaptureStorage::Memory(_) => {
                let previous = match std::mem::replace(&mut self.storage, CaptureStorage::Poisoned) {
                    CaptureStorage::Memory(buf) => buf.freeze(),
                    _ => {
                        return Err(Self::replay_error(
                            "request body storage changed while starting disk spill",
                        ))
                    }
                };
                let incoming = data.clone();
                let spill = tokio::task::spawn_blocking(move || create_spill_file(&previous, &incoming))
                    .await
                    .map_err(|_| Self::replay_error("request body spill task failed"))?
                    .map_err(|error| {
                        PingoraError::because(ErrorType::InternalError, "create request body spill file", error)
                    })?;
                self.storage = CaptureStorage::Disk {
                    temp_file: spill.0,
                    file: spill.1,
                };
            }
            CaptureStorage::Disk { file, .. } => {
                let file = Arc::clone(file);
                let offset = self.captured_len;
                let incoming = data.clone();
                tokio::task::spawn_blocking(move || write_all_at(&file, &incoming, offset))
                    .await
                    .map_err(|_| Self::replay_error("request body spill write task failed"))?
                    .map_err(|error| {
                        PingoraError::because(ErrorType::InternalError, "write request body spill file", error)
                    })?;
            }
            CaptureStorage::FinishedMemory(_) | CaptureStorage::FinishedDisk { .. } => {
                return Err(Self::replay_error("request body capture continued after finalization"))
            }
            CaptureStorage::Poisoned => {
                return Err(Self::replay_error(
                    "request body storage is unusable after a failed spill",
                ))
            }
        }
        self.captured_len = next_len;
        Ok(())
    }

    async fn finish(&mut self) -> PingoraResult<()> {
        if self.finished {
            return Ok(());
        }
        let storage = std::mem::replace(&mut self.storage, CaptureStorage::Poisoned);
        match storage {
            CaptureStorage::Memory(buf) => {
                let body = buf.freeze();
                self.publish(BodySource::Memory(body.clone()))?;
                self.storage = CaptureStorage::FinishedMemory(body);
            }
            CaptureStorage::Disk { temp_file, file } => {
                self.publish(BodySource::Disk {
                    file: Arc::clone(&file),
                    len: self.captured_len,
                })?;
                self.storage = CaptureStorage::FinishedDisk { temp_file, file };
            }
            CaptureStorage::FinishedMemory(body) => self.storage = CaptureStorage::FinishedMemory(body),
            CaptureStorage::FinishedDisk { temp_file, file } => {
                self.storage = CaptureStorage::FinishedDisk { temp_file, file };
            }
            CaptureStorage::Poisoned => {
                return Err(Self::replay_error(
                    "request body storage cannot be finalized after a failed spill",
                ))
            }
        }
        self.finished = true;
        Ok(())
    }

    async fn rewind(&mut self) -> PingoraResult<()> {
        if !self.finished {
            return Err(Self::replay_error(
                "request body replay started before capture completed",
            ));
        }
        let view = BodyStorageView::new(Arc::clone(&self.view));
        self.replay_snapshot = Some(
            view.snapshot()
                .map_err(|_| Self::replay_error("request body replay source is unavailable after finalization"))?
                .0,
        );
        self.replay_cursor = 0;
        self.replay_invalid = false;
        Ok(())
    }

    async fn next_chunk(&mut self, max_bytes: usize) -> PingoraResult<Option<Bytes>> {
        if !self.finished {
            return Err(Self::replay_error(
                "request body replay started before capture completed",
            ));
        }
        if self.replay_invalid {
            return Err(Self::replay_error(
                "request body replay cursor was advanced beyond captured data",
            ));
        }
        if max_bytes == 0 {
            return Err(Self::replay_error("request body replay requested a zero-sized chunk"));
        }
        let snapshot = self
            .replay_snapshot
            .as_ref()
            .ok_or_else(|| Self::replay_error("request body replay requires rewind"))?;
        if self.replay_cursor == snapshot.len {
            return Ok(None);
        }
        if self.replay_cursor > snapshot.len {
            return Err(Self::replay_error("request body replay cursor exceeds captured length"));
        }
        let wanted = usize::try_from((snapshot.len - self.replay_cursor).min(max_bytes as u64))
            .map_err(|_| Self::replay_error("request body replay chunk length does not fit in usize"))?;
        let segment = locate_segment(snapshot, self.replay_cursor)
            .map_err(|_| Self::replay_error("request body replay snapshot is inconsistent"))?;
        let wanted = wanted.min(
            usize::try_from(segment.remaining)
                .map_err(|_| Self::replay_error("request body replay segment length overflow"))?,
        );
        let chunk = match segment.segment {
            Segment::SourceRange { source, offset, .. } => match source.as_ref() {
                BodySource::Memory(bytes) => {
                    let start = usize::try_from(offset + segment.local_offset)
                        .map_err(|_| Self::replay_error("request body replay offset overflow"))?;
                    bytes.slice(start..start + wanted)
                }
                BodySource::Disk { file, .. } => {
                    let file = Arc::clone(file);
                    let offset = offset
                        .checked_add(segment.local_offset)
                        .ok_or_else(|| Self::replay_error("request body replay offset overflow"))?;
                    let bytes = tokio::task::spawn_blocking(move || read_at_most(&file, wanted, offset))
                        .await
                        .map_err(|_| Self::replay_error("request body replay read task failed"))?
                        .map_err(|error| {
                            PingoraError::because(ErrorType::InternalError, "read request body spill file", error)
                        })?;
                    if bytes.len() != wanted {
                        return Err(Self::replay_error(format!(
                            "request body replay truncated: served {} of {} captured bytes",
                            self.replay_cursor, snapshot.len
                        )));
                    }
                    Bytes::from(bytes)
                }
            },
            Segment::Inline(bytes) => {
                let start = usize::try_from(segment.local_offset)
                    .map_err(|_| Self::replay_error("request body replay offset overflow"))?;
                bytes.slice(start..start + wanted)
            }
        };
        Ok(Some(chunk))
    }

    fn consume(&mut self, bytes: usize) {
        let Some(snapshot) = self.replay_snapshot.as_ref() else {
            self.replay_invalid = true;
            return;
        };
        let Ok(bytes) = u64::try_from(bytes) else {
            self.replay_invalid = true;
            return;
        };
        let Some(next) = self.replay_cursor.checked_add(bytes) else {
            self.replay_invalid = true;
            return;
        };
        if bytes == 0 || next > snapshot.len {
            self.replay_invalid = true;
            return;
        }
        self.replay_cursor = next;
    }
}

struct LocatedSegment<'a> {
    segment: &'a Segment,
    local_offset: u64,
    remaining: u64,
}

fn locate_segment(snapshot: &BodySnapshot, logical_offset: u64) -> Result<LocatedSegment<'_>, BodyOperationError> {
    let mut base = 0u64;
    for segment in &snapshot.segments {
        let len = segment.len()?;
        let end = base
            .checked_add(len)
            .ok_or(BodyOperationError::Body(BodyReadError::Incomplete))?;
        if logical_offset < end {
            let local_offset = logical_offset - base;
            return Ok(LocatedSegment {
                segment,
                local_offset,
                remaining: len - local_offset,
            });
        }
        base = end;
    }
    Err(BodyOperationError::Body(BodyReadError::Incomplete))
}

fn create_spill_file(previous: &Bytes, incoming: &Bytes) -> io::Result<(NamedTempFile, Arc<File>)> {
    let temp_file = NamedTempFile::new()?;
    let file = Arc::new(temp_file.as_file().try_clone()?);
    write_all_at(&file, previous, 0)?;
    write_all_at(&file, incoming, previous.len() as u64)?;
    Ok((temp_file, file))
}

fn write_all_at(file: &File, mut data: &[u8], mut offset: u64) -> io::Result<()> {
    while !data.is_empty() {
        let written = file.write_at(data, offset)?;
        if written == 0 {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "request body spill write returned zero bytes",
            ));
        }
        data = &data[written..];
        offset = offset
            .checked_add(written as u64)
            .ok_or_else(|| io::Error::other("request body spill offset overflow"))?;
    }
    Ok(())
}

fn read_at_most(file: &File, len: usize, offset: u64) -> io::Result<Vec<u8>> {
    read_at_most_with(len, offset, |buf, position| file.read_at(buf, position))
}

fn read_at_most_with(
    len: usize,
    offset: u64,
    mut read_at: impl FnMut(&mut [u8], u64) -> io::Result<usize>,
) -> io::Result<Vec<u8>> {
    let mut buf = vec![0; len];
    let mut filled = 0usize;
    while filled < len {
        let position = offset
            .checked_add(filled as u64)
            .ok_or_else(|| io::Error::other("request body read offset overflow"))?;
        let read = read_at(&mut buf[filled..], position)?;
        if read == 0 {
            break;
        }
        filled = filled
            .checked_add(read)
            .filter(|filled| *filled <= len)
            .ok_or_else(|| io::Error::other("request body read returned an invalid length"))?;
    }
    buf.truncate(filled);
    Ok(buf)
}

async fn read_disk_body(file: Arc<File>, len: u64) -> BodyReadResult {
    let len = usize::try_from(len).map_err(|_| BodyReadError::Incomplete)?;
    tokio::task::spawn_blocking(move || {
        let mut body = vec![0; len];
        let mut filled = 0usize;
        while filled < body.len() {
            let read = file.read_at(&mut body[filled..], filled as u64)?;
            if read == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "request body spill file ended before captured length",
                ));
            }
            filled += read;
        }
        Ok::<_, io::Error>(Bytes::from(body))
    })
    .await
    .map_err(|_| BodyReadError::Incomplete)?
    .map_err(|_| BodyReadError::Incomplete)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::{poll_fn, Future};
    use std::task::Poll;

    async fn capture(storage: &mut BodyStorage, chunks: &[&[u8]]) {
        for chunk in chunks {
            storage.write(&Bytes::copy_from_slice(chunk)).await.unwrap();
        }
        storage.finish().await.unwrap();
    }

    async fn replay_all(storage: &mut BodyStorage, chunk_size: usize) -> Bytes {
        storage.rewind().await.unwrap();
        let mut body = BytesMut::new();
        while let Some(chunk) = storage.next_chunk(chunk_size).await.unwrap() {
            let len = chunk.len();
            body.extend_from_slice(&chunk);
            storage.consume(len);
        }
        body.freeze()
    }

    async fn read_snapshot(mut reader: Box<dyn RequestBodySnapshotReader>) -> (Vec<usize>, Bytes) {
        let mut chunk_lengths = Vec::new();
        let mut body = BytesMut::new();
        while let Some(chunk) = reader.next_chunk().await.unwrap() {
            chunk_lengths.push(chunk.len());
            body.extend_from_slice(&chunk);
        }
        (chunk_lengths, body.freeze())
    }

    fn disk_file(storage: &BodyStorage) -> (&NamedTempFile, &Arc<File>) {
        match &storage.storage {
            CaptureStorage::FinishedDisk { temp_file, file } => (temp_file, file),
            _ => panic!("test requires a finished disk-backed body"),
        }
    }

    #[tokio::test]
    async fn memory_and_disk_text_operations_cross_scan_boundaries() {
        let mut raw = vec![b'x'; SCAN_CHUNK_SIZE - 1];
        raw.extend_from_slice("€needle".as_bytes());
        raw.extend_from_slice(&[0xf0, 0x90]);
        for memory_limit in [raw.len() + 1, 1] {
            let (mut storage, view) = BodyStorage::with_memory_limit(memory_limit);
            capture(&mut storage, &[&raw]).await;
            let contains = view
                .text_operation(BodyTextOperation::Contains("€needle�".to_string()), u64::MAX)
                .await
                .unwrap();
            assert_eq!(contains.value, BodyTextResult::Bool(true));
            assert_eq!(contains.bytes_scanned, raw.len() as u64);
            let len = view.text_operation(BodyTextOperation::CharLen, u64::MAX).await.unwrap();
            assert_eq!(
                len.value,
                BodyTextResult::CharLen(String::from_utf8_lossy(&raw).chars().count() as u64)
            );
        }
    }

    #[tokio::test]
    async fn disk_scan_does_not_materialize() {
        let (mut storage, view) = BodyStorage::with_memory_limit(1);
        capture(&mut storage, &[b"prefix-body-suffix"]).await;
        assert_eq!(
            view.text_operation(BodyTextOperation::StartsWith("prefix".to_string()), u64::MAX)
                .await
                .unwrap()
                .value,
            BodyTextResult::Bool(true)
        );
        assert!(!view.is_materialized());
        let expected: [u8; 32] = Sha256::digest(b"prefix-body-suffix").into();
        assert_eq!(view.sha256_scan(u64::MAX).await.unwrap().value, expected);
        assert!(!view.is_materialized());
    }

    #[tokio::test]
    async fn snapshot_reader_streams_memory_in_bounded_chunks() {
        let body = vec![b'm'; SCAN_CHUNK_SIZE * 2 + 17];
        let (mut storage, view) = BodyStorage::with_memory_limit(body.len() + 1);
        capture(&mut storage, &[&body]).await;

        let reader = view.snapshot_reader().unwrap();
        assert_eq!(reader.len(), body.len() as u64);
        let (chunk_lengths, streamed) = read_snapshot(reader).await;

        assert_eq!(chunk_lengths, vec![SCAN_CHUNK_SIZE, SCAN_CHUNK_SIZE, 17]);
        assert_eq!(streamed.as_ref(), body.as_slice());
        assert!(!view.is_materialized());
    }

    #[tokio::test]
    async fn snapshot_reader_streams_spill_without_materializing() {
        let body = vec![b'd'; SCAN_CHUNK_SIZE + 11];
        let (mut storage, view) = BodyStorage::with_memory_limit(1);
        capture(&mut storage, &[&body]).await;

        let reader = view.snapshot_reader().unwrap();
        assert_eq!(reader.len(), body.len() as u64);
        let (chunk_lengths, streamed) = read_snapshot(reader).await;

        assert_eq!(chunk_lengths, vec![SCAN_CHUNK_SIZE, 11]);
        assert_eq!(streamed.as_ref(), body.as_slice());
        assert!(!view.is_materialized());
    }

    #[tokio::test]
    async fn snapshot_reader_remains_stable_across_mutation() {
        let (mut storage, view) = BodyStorage::with_memory_limit(1);
        capture(&mut storage, &[b"original"]).await;
        let old_reader = view.snapshot_reader().unwrap();

        view.prepend(Bytes::from_static(b"new-")).unwrap();
        view.remove(4..12).unwrap();

        assert_eq!(read_snapshot(old_reader).await.1, Bytes::from_static(b"original"));
        assert_eq!(
            read_snapshot(view.snapshot_reader().unwrap()).await.1,
            Bytes::from_static(b"new-")
        );
        assert!(!view.is_materialized());
    }

    #[tokio::test]
    async fn snapshot_reader_handles_empty_body_without_chunks() {
        let (mut storage, view) = BodyStorage::with_memory_limit(1);
        capture(&mut storage, &[]).await;

        let reader = view.snapshot_reader().unwrap();
        assert_eq!(reader.len(), 0);
        assert_eq!(read_snapshot(reader).await, (Vec::new(), Bytes::new()));
        assert!(!view.is_materialized());
    }

    #[tokio::test]
    async fn source_range_mutations_preserve_spill_and_replay() {
        let (mut storage, view) = BodyStorage::with_memory_limit(1);
        capture(&mut storage, &[b"alpha-beta-alpha"]).await;
        view.prepend(Bytes::from_static(b"<")).unwrap();
        view.append(Bytes::from_static(b">")).unwrap();
        view.remove(7..12).unwrap();
        assert!(!view.is_materialized());
        assert_eq!(replay_all(&mut storage, 3).await, Bytes::from_static(b"<alpha-alpha>"));
        assert_eq!(replay_all(&mut storage, 2).await, Bytes::from_static(b"<alpha-alpha>"));
    }

    #[tokio::test]
    async fn literal_replace_is_streaming_transactional_and_stable_for_retry() {
        let (mut storage, view) = BodyStorage::with_limits(1, 64);
        capture(&mut storage, &[b"ababa--aba"]).await;
        let result = view
            .replace_literal(Bytes::from_static(b"aba"), Bytes::from_static(b"X"), 64)
            .await
            .unwrap();
        assert_eq!(result.bytes_scanned, 10);
        assert_eq!(replay_all(&mut storage, 1).await, Bytes::from_static(b"Xba--X"));
        assert_eq!(replay_all(&mut storage, 4).await, Bytes::from_static(b"Xba--X"));
        assert!(!view.is_materialized());
    }

    #[tokio::test]
    async fn physical_cap_failure_rolls_back_without_changing_replay() {
        let (mut storage, view) = BodyStorage::with_limits(1, 6);
        capture(&mut storage, &[b"abcabc"]).await;
        let error = view
            .replace_literal(Bytes::from_static(b"a"), Bytes::from_static(b"long"), 6)
            .await
            .unwrap_err();
        assert_eq!(
            error,
            BodyOperationError::Body(BodyReadError::BufferLimitExceeded { limit: 6 })
        );
        assert_eq!(replay_all(&mut storage, 2).await, Bytes::from_static(b"abcabc"));
    }

    #[tokio::test]
    async fn scan_limit_reports_actual_early_exit_and_hard_limit() {
        let (mut storage, view) = BodyStorage::with_memory_limit(64);
        capture(&mut storage, &[b"needle and trailing bytes"]).await;
        let early = view
            .text_operation(BodyTextOperation::Contains("needle".to_string()), 3)
            .await
            .unwrap_err();
        assert_eq!(early, BodyOperationError::ScanLimitExceeded { limit: 3 });
        let found = view
            .text_operation(BodyTextOperation::Contains("needle".to_string()), 6)
            .await
            .unwrap();
        assert_eq!(found.value, BodyTextResult::Bool(true));
        assert_eq!(found.bytes_scanned, 6);
    }

    #[tokio::test]
    async fn empty_text_patterns_and_empty_metadata_do_not_scan() {
        let (mut storage, view) = BodyStorage::with_memory_limit(64);
        capture(&mut storage, &[b"non-empty"]).await;
        for operation in [
            BodyTextOperation::StartsWith(String::new()),
            BodyTextOperation::EndsWith(String::new()),
            BodyTextOperation::Contains(String::new()),
        ] {
            let result = view.text_operation(operation, 0).await.unwrap();
            assert_eq!(result.value, BodyTextResult::Bool(true));
            assert_eq!(result.bytes_scanned, 0);
        }
        let result = view.text_operation(BodyTextOperation::IsEmpty, 0).await.unwrap();
        assert_eq!(result.value, BodyTextResult::Bool(false));
        assert_eq!(result.bytes_scanned, 0);
    }

    #[tokio::test]
    async fn rewind_keeps_snapshot_stable_during_later_mutation() {
        let (mut storage, view) = BodyStorage::with_memory_limit(64);
        capture(&mut storage, &[b"original"]).await;
        storage.rewind().await.unwrap();
        view.overwrite_body(Bytes::from_static(b"new")).unwrap();
        let mut old = BytesMut::new();
        while let Some(chunk) = storage.next_chunk(2).await.unwrap() {
            let len = chunk.len();
            old.extend_from_slice(&chunk);
            storage.consume(len);
        }
        assert_eq!(old.freeze(), Bytes::from_static(b"original"));
        assert_eq!(replay_all(&mut storage, 2).await, Bytes::from_static(b"new"));
    }

    #[tokio::test]
    async fn replace_slice_compatibility_is_atomic() {
        let (mut storage, view) = BodyStorage::with_limits(1, 6);
        capture(&mut storage, &[b"abcdef"]).await;
        assert_eq!(
            view.replace_slice(1..2, Bytes::from_static(b"123456")).await,
            Err(BodyReadError::BufferLimitExceeded { limit: 6 })
        );
        assert_eq!(replay_all(&mut storage, 2).await, Bytes::from_static(b"abcdef"));
        assert_eq!(
            view.replace_slice(1..2, Bytes::from_static(b"Z")).await.unwrap(),
            Bytes::from_static(b"aZcdef")
        );
    }

    #[tokio::test]
    async fn segment_limit_failure_is_atomic() {
        let (mut storage, view) = BodyStorage::with_limits(64, u64::MAX);
        capture(&mut storage, &[b"x"]).await;
        for _ in 1..MAX_BODY_SEGMENTS {
            view.append(Bytes::from_static(b"x")).unwrap();
        }
        assert_eq!(view.len().unwrap(), MAX_BODY_SEGMENTS as u64);
        assert_eq!(
            view.append(Bytes::from_static(b"x")),
            Err(BodyOperationError::SegmentLimitExceeded {
                limit: MAX_BODY_SEGMENTS
            })
        );
        assert_eq!(view.len().unwrap(), MAX_BODY_SEGMENTS as u64);
    }

    struct SlowCountingScanner {
        chunks: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl RawBodyScanner for SlowCountingScanner {
        type Output = ();

        fn push(&mut self, _chunk: &[u8]) -> Result<ScanControl, BodyOperationError> {
            self.chunks.fetch_add(1, Ordering::AcqRel);
            std::thread::sleep(std::time::Duration::from_millis(10));
            Ok(ScanControl::Continue)
        }

        fn finish(self) -> Result<Self::Output, BodyOperationError> {
            Ok(())
        }
    }

    struct BlockingContextScanner {
        caller: std::thread::ThreadId,
        ran_off_caller: bool,
    }

    impl RawBodyScanner for BlockingContextScanner {
        type Output = bool;

        fn requires_blocking_context(&self) -> bool {
            true
        }

        fn push(&mut self, _chunk: &[u8]) -> Result<ScanControl, BodyOperationError> {
            self.ran_off_caller = std::thread::current().id() != self.caller;
            Ok(ScanControl::Continue)
        }

        fn finish(self) -> Result<Self::Output, BodyOperationError> {
            Ok(self.ran_off_caller)
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn blocking_scanner_uses_blocking_pool_for_small_memory_body() {
        let (mut storage, view) = BodyStorage::with_memory_limit(64);
        capture(&mut storage, &[b"small"]).await;
        let result = view
            .scan_raw(
                BlockingContextScanner {
                    caller: std::thread::current().id(),
                    ran_off_caller: false,
                },
                u64::MAX,
            )
            .await
            .unwrap();
        assert!(result.value);
    }

    #[tokio::test]
    async fn cancelled_disk_scan_stops_between_chunks() {
        let (mut storage, view) = BodyStorage::with_memory_limit(1);
        let body = vec![b'x'; SCAN_CHUNK_SIZE * 16];
        capture(&mut storage, &[&body]).await;
        let chunks = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let task_view = Arc::clone(&view);
        let task_chunks = Arc::clone(&chunks);
        let task = tokio::spawn(async move {
            task_view
                .scan_raw(SlowCountingScanner { chunks: task_chunks }, u64::MAX)
                .await
        });
        while chunks.load(Ordering::Acquire) == 0 {
            tokio::task::yield_now().await;
        }
        task.abort();
        let _ = task.await;
        tokio::time::sleep(std::time::Duration::from_millis(40)).await;
        assert!(
            chunks.load(Ordering::Acquire) < 16,
            "cancelled disk scan must not consume the complete snapshot"
        );
    }

    #[tokio::test]
    async fn cancelled_disk_peek_replays_same_chunk() {
        let (mut storage, _view) = BodyStorage::with_memory_limit(1);
        let body = vec![b'x'; 256 * 1024];
        capture(&mut storage, &[&body]).await;
        storage.rewind().await.unwrap();
        let mut peek = Box::pin(storage.next_chunk(64 * 1024));
        let completed = poll_fn(|cx| match peek.as_mut().poll(cx) {
            Poll::Pending => Poll::Ready(None),
            Poll::Ready(result) => Poll::Ready(Some(result)),
        })
        .await;
        drop(peek);
        let expected = Bytes::copy_from_slice(&body[..64 * 1024]);
        if let Some(result) = completed {
            assert_eq!(result.unwrap(), Some(expected.clone()));
        }
        assert_eq!(storage.next_chunk(64 * 1024).await.unwrap(), Some(expected));
    }

    #[tokio::test]
    async fn truncated_spill_fails_for_scan_and_replay() {
        let (mut storage, view) = BodyStorage::with_memory_limit(1);
        capture(&mut storage, &[b"abcdefghij"]).await;
        let (_, file) = disk_file(&storage);
        file.set_len(4).unwrap();
        assert_eq!(
            view.sha256_scan(u64::MAX).await.unwrap_err(),
            BodyOperationError::Body(BodyReadError::Incomplete)
        );
        storage.rewind().await.unwrap();
        assert!(storage.next_chunk(64).await.is_err());
    }

    #[test]
    fn positional_read_retries_short_reads_until_filled() {
        let source = b"abcdefgh";
        let mut calls = 0usize;
        let body = read_at_most_with(source.len(), 0, |target, offset| {
            calls += 1;
            let start = usize::try_from(offset).unwrap();
            let count = target.len().min(2).min(source.len() - start);
            target[..count].copy_from_slice(&source[start..start + count]);
            Ok(count)
        })
        .unwrap();
        assert_eq!(body, source);
        assert_eq!(calls, 4);
    }

    #[tokio::test]
    async fn dropping_storage_unlinks_spill_while_snapshot_view_stays_readable() {
        let (mut storage, view) = BodyStorage::with_memory_limit(1);
        capture(&mut storage, &[b"persistent-body"]).await;
        let path = disk_file(&storage).0.path().to_path_buf();
        drop(storage);
        assert!(!path.exists());
        assert_eq!(
            view.read_range(0, 64, 64).await.unwrap().value,
            Bytes::from_static(b"persistent-body")
        );
    }
}
