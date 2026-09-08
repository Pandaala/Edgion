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
use std::time::Duration;
use tempfile::tempfile;
use tokio::sync::{OnceCell, Semaphore};
use tokio::task::JoinHandle;
use tokio::time::{timeout_at, Instant};

use super::config::DEFAULT_STORAGE_OPERATION_TIMEOUT;
use super::operation::{
    BodyOperationError, BodyScanResult, BodyTextOperation, BodyTextResult, BytesScanner, LossyTextScanner,
    RawBodyScanner, ScanControl, TextOperationScanner, MAX_BODY_SEGMENTS,
};
use crate::plugins::runtime::traits::session::{BodyReadError, BodyReadResult, RequestBodySnapshotReader};

const SCAN_CHUNK_SIZE: usize = 64 * 1024;
const SCAN_CONCURRENCY: usize = 32;
const STORAGE_IO_CONCURRENCY: usize = 32;

fn scan_limiter() -> &'static Arc<Semaphore> {
    static LIMITER: OnceLock<Arc<Semaphore>> = OnceLock::new();
    LIMITER.get_or_init(|| Arc::new(Semaphore::new(SCAN_CONCURRENCY)))
}

pub(super) async fn run_blocking_scan<T, F>(work: F) -> Result<T, BodyOperationError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, BodyOperationError> + Send + 'static,
{
    run_blocking_scan_with_limiter(work, Arc::clone(scan_limiter())).await
}

async fn run_blocking_scan_with_limiter<T, F>(work: F, limiter: Arc<Semaphore>) -> Result<T, BodyOperationError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, BodyOperationError> + Send + 'static,
{
    let permit = limiter
        .acquire_owned()
        .await
        .map_err(|_| BodyOperationError::Body(BodyReadError::Incomplete))?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        work()
    })
    .await
    .map_err(|_| BodyOperationError::Body(BodyReadError::Incomplete))?
}

fn storage_io_limiter() -> &'static Arc<Semaphore> {
    static LIMITER: OnceLock<Arc<Semaphore>> = OnceLock::new();
    LIMITER.get_or_init(|| Arc::new(Semaphore::new(STORAGE_IO_CONCURRENCY)))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StorageIoOperation {
    SpillCreate,
    SpillWrite,
    ReplayRead,
    MaterializeRead,
    SnapshotRead,
}

impl StorageIoOperation {
    const fn as_metric_label(self) -> &'static str {
        match self {
            Self::SpillCreate => "spill_create",
            Self::SpillWrite => "spill_write",
            Self::ReplayRead => "replay_read",
            Self::MaterializeRead => "materialize_read",
            Self::SnapshotRead => "snapshot_read",
        }
    }
}

#[cfg(test)]
tokio::task_local! {
    pub(crate) static STORAGE_IO_TEST_HOOK: Arc<dyn Fn(StorageIoOperation) + Send + Sync>;
}

#[derive(Debug)]
enum StorageIoError {
    AdmissionTimeout,
    AdmissionClosed,
    ExecutionTimeout,
    Join(tokio::task::JoinError),
    Io(io::Error),
}

impl StorageIoError {
    const fn metric_reason(&self) -> &'static str {
        match self {
            Self::AdmissionTimeout => "admission_timeout",
            Self::AdmissionClosed => "admission_closed",
            Self::ExecutionTimeout => "execution_timeout",
            Self::Join(_) => "join_error",
            Self::Io(_) => "io_error",
        }
    }
}

struct StorageIoInFlight;

impl StorageIoInFlight {
    fn new() -> Self {
        crate::observe::metrics::adjust_request_body_storage_io_inflight(1.0);
        Self
    }
}

impl Drop for StorageIoInFlight {
    fn drop(&mut self) {
        crate::observe::metrics::adjust_request_body_storage_io_inflight(-1.0);
    }
}

struct AbortBlockingTask<T> {
    handle: Option<JoinHandle<T>>,
}

impl<T> AbortBlockingTask<T> {
    fn new(handle: JoinHandle<T>) -> Self {
        Self { handle: Some(handle) }
    }

    fn handle_mut(&mut self) -> &mut JoinHandle<T> {
        self.handle.as_mut().expect("blocking task handle is present")
    }

    fn disarm(&mut self) {
        self.handle.take();
    }
}

impl<T> Drop for AbortBlockingTask<T> {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.abort();
        }
    }
}

async fn run_storage_io<T, F>(timeout: Duration, operation: StorageIoOperation, work: F) -> Result<T, StorageIoError>
where
    T: Send + 'static,
    F: FnOnce() -> io::Result<T> + Send + 'static,
{
    run_storage_io_with_limiter(Arc::clone(storage_io_limiter()), timeout, operation, work).await
}

async fn run_storage_io_with_limiter<T, F>(
    limiter: Arc<Semaphore>,
    timeout: Duration,
    operation: StorageIoOperation,
    work: F,
) -> Result<T, StorageIoError>
where
    T: Send + 'static,
    F: FnOnce() -> io::Result<T> + Send + 'static,
{
    run_storage_io_retained_with_limiter(limiter, timeout, operation, work)
        .await
        .map(|output| output.value)
}

/// Mirror reads keep the canonical IO permit with their raw output, including
/// completed output waiting in a JoinHandle. Other consumers release it on return.
struct StorageIoOutput<T> {
    value: T,
    _permit: tokio::sync::OwnedSemaphorePermit,
}

impl<T: AsRef<[u8]>> AsRef<[u8]> for StorageIoOutput<T> {
    fn as_ref(&self) -> &[u8] {
        self.value.as_ref()
    }
}

async fn run_storage_io_retained_with_limiter<T, F>(
    limiter: Arc<Semaphore>,
    timeout: Duration,
    operation: StorageIoOperation,
    work: F,
) -> Result<StorageIoOutput<T>, StorageIoError>
where
    T: Send + 'static,
    F: FnOnce() -> io::Result<T> + Send + 'static,
{
    let deadline = Instant::now() + timeout;
    let permit = match timeout_at(deadline, limiter.acquire_owned()).await {
        Err(_) => {
            let error = StorageIoError::AdmissionTimeout;
            record_storage_io_failure(operation, &error);
            return Err(error);
        }
        Ok(Err(_)) => {
            let error = StorageIoError::AdmissionClosed;
            record_storage_io_failure(operation, &error);
            return Err(error);
        }
        Ok(Ok(permit)) => permit,
    };

    #[cfg(test)]
    let test_hook = STORAGE_IO_TEST_HOOK.try_with(Arc::clone).ok();
    let inflight = StorageIoInFlight::new();
    let mut task = AbortBlockingTask::new(tokio::task::spawn_blocking(move || {
        let _inflight = inflight;
        #[cfg(test)]
        if let Some(hook) = test_hook {
            hook(operation);
        }
        work().map(|value| StorageIoOutput { value, _permit: permit })
    }));

    let result = match timeout_at(deadline, task.handle_mut()).await {
        Err(_) => Err(StorageIoError::ExecutionTimeout),
        Ok(Err(error)) => Err(StorageIoError::Join(error)),
        Ok(Ok(Err(error))) => Err(StorageIoError::Io(error)),
        Ok(Ok(Ok(value))) => Ok(value),
    };
    if !matches!(&result, Err(StorageIoError::ExecutionTimeout)) {
        task.disarm();
    }
    if let Err(error) = &result {
        record_storage_io_failure(operation, error);
    }
    result
}

fn record_storage_io_failure(operation: StorageIoOperation, error: &StorageIoError) {
    crate::observe::metrics::record_request_body_storage_io_failure(operation.as_metric_label(), error.metric_reason());
}

fn storage_io_pingora_error(context: &'static str, error: StorageIoError) -> Box<PingoraError> {
    match error {
        StorageIoError::Io(error) => PingoraError::because(ErrorType::InternalError, context, error),
        StorageIoError::Join(error) => PingoraError::because(ErrorType::InternalError, context, error),
        StorageIoError::AdmissionTimeout | StorageIoError::AdmissionClosed | StorageIoError::ExecutionTimeout => {
            PingoraError::explain(ErrorType::InternalError, context)
        }
    }
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

struct BodySnapshot {
    segments: Vec<Segment>,
    len: u64,
    sha256: OnceCell<[u8; 32]>,
}

impl BodySnapshot {
    fn empty() -> Self {
        Self {
            segments: Vec::new(),
            len: 0,
            sha256: OnceCell::new(),
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
            sha256: OnceCell::new(),
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
            sha256: OnceCell::new(),
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
        Ok(Self {
            segments,
            len,
            sha256: OnceCell::new(),
        })
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
    let mut normalized: Vec<Segment> = Vec::with_capacity(segments.len().min(MAX_BODY_SEGMENTS));
    for segment in segments {
        push_normalized_segment(&mut normalized, segment)?;
    }
    Ok(normalized)
}

fn push_normalized_segment(output: &mut Vec<Segment>, segment: Segment) -> Result<(), BodyOperationError> {
    if segment.len()? == 0 {
        return Ok(());
    }
    if let (
        Some(Segment::SourceRange {
            source: previous_source,
            offset: previous_offset,
            len: previous_len,
        }),
        Segment::SourceRange { source, offset, len },
    ) = (output.last_mut(), &segment)
    {
        let previous_end = previous_offset
            .checked_add(*previous_len)
            .ok_or(BodyOperationError::Body(BodyReadError::Incomplete))?;
        if Arc::ptr_eq(previous_source, source) && previous_end == *offset {
            *previous_len = previous_len
                .checked_add(*len)
                .ok_or(BodyOperationError::Body(BodyReadError::Incomplete))?;
            return Ok(());
        }
    }
    if output.len() == MAX_BODY_SEGMENTS {
        return Err(BodyOperationError::SegmentLimitExceeded {
            limit: MAX_BODY_SEGMENTS,
        });
    }
    output.push(segment);
    Ok(())
}

/// Monotonic cursor used by batched edits so each source segment is visited at
/// most once, independent of the number of replacement ranges.
struct SegmentSplicer<'a> {
    snapshot: &'a BodySnapshot,
    segment_index: usize,
    segment_offset: u64,
    logical_offset: u64,
}

impl<'a> SegmentSplicer<'a> {
    fn new(snapshot: &'a BodySnapshot) -> Self {
        Self {
            snapshot,
            segment_index: 0,
            segment_offset: 0,
            logical_offset: 0,
        }
    }

    fn append_until(&mut self, end: u64, output: &mut Vec<Segment>) -> Result<(), BodyOperationError> {
        self.advance_until(end, Some(output))
    }

    fn skip_until(&mut self, end: u64) -> Result<(), BodyOperationError> {
        self.advance_until(end, None)
    }

    fn advance_until(&mut self, end: u64, mut output: Option<&mut Vec<Segment>>) -> Result<(), BodyOperationError> {
        if end < self.logical_offset || end > self.snapshot.len {
            return Err(BodyOperationError::InvalidRange);
        }
        while self.logical_offset < end {
            let segment = self
                .snapshot
                .segments
                .get(self.segment_index)
                .ok_or(BodyOperationError::Body(BodyReadError::Incomplete))?;
            let segment_len = segment.len()?;
            if self.segment_offset >= segment_len {
                return Err(BodyOperationError::Body(BodyReadError::Incomplete));
            }
            let consumed = (end - self.logical_offset).min(segment_len - self.segment_offset);
            if let Some(output) = output.as_deref_mut() {
                push_normalized_segment(output, slice_segment(segment, self.segment_offset, consumed)?)?;
            }
            self.segment_offset = self
                .segment_offset
                .checked_add(consumed)
                .ok_or(BodyOperationError::Body(BodyReadError::Incomplete))?;
            self.logical_offset = self
                .logical_offset
                .checked_add(consumed)
                .ok_or(BodyOperationError::Body(BodyReadError::Incomplete))?;
            if self.segment_offset == segment_len {
                self.segment_index += 1;
                self.segment_offset = 0;
            }
        }
        Ok(())
    }
}

#[derive(Default)]
struct MutationState {
    snapshot: Option<Arc<BodySnapshot>>,
    generation: u64,
}

struct InitialBody {
    source: Arc<BodySource>,
    snapshot: Arc<BodySnapshot>,
}

struct BodyStorageViewInner {
    initial: OnceLock<InitialBody>,
    materialized: OnceCell<Bytes>,
    mutation: RwLock<MutationState>,
    max_body_size_bytes: u64,
    storage_operation_timeout: Duration,
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
        let initial = self.inner.initial.get().ok_or(BodyReadError::Incomplete)?;
        Ok((Arc::clone(&initial.snapshot), mutation.generation))
    }

    fn publish(&self, expected_generation: u64, snapshot: BodySnapshot) -> Result<u64, BodyOperationError> {
        self.publish_arc(expected_generation, Arc::new(snapshot))
    }

    fn publish_arc(&self, expected_generation: u64, snapshot: Arc<BodySnapshot>) -> Result<u64, BodyOperationError> {
        if snapshot.len > self.inner.max_body_size_bytes {
            return Err(BodyOperationError::Body(BodyReadError::BufferLimitExceeded {
                limit: self.inner.max_body_size_bytes,
            }));
        }
        let mut mutation = self.inner.mutation.write();
        if mutation.generation != expected_generation {
            // Distinct from `Incomplete`: a lost update and a truncated body
            // demand opposite handling, and offsets computed against the stale
            // snapshot must never be spliced into the newer one.
            return Err(BodyOperationError::Body(BodyReadError::GenerationConflict));
        }
        let committed_generation = mutation
            .generation
            .checked_add(1)
            .ok_or(BodyOperationError::Body(BodyReadError::Incomplete))?;
        mutation.generation = committed_generation;
        mutation.snapshot = Some(snapshot);
        Ok(committed_generation)
    }

    pub async fn read_all(&self) -> BodyReadResult {
        let mutation = self.inner.mutation.read().snapshot.clone();
        if let Some(snapshot) = mutation {
            return self.materialize_snapshot(snapshot).await;
        }

        let initial = self.inner.initial.get().ok_or(BodyReadError::Incomplete)?;
        match initial.source.as_ref() {
            BodySource::Memory(body) => Ok(body.clone()),
            BodySource::Disk { file, len } => {
                let body = self
                    .inner
                    .materialized
                    .get_or_try_init(|| read_disk_body(Arc::clone(file), *len, self.inner.storage_operation_timeout))
                    .await?;
                Ok(body.clone())
            }
        }
    }

    async fn materialize_snapshot(&self, snapshot: Arc<BodySnapshot>) -> BodyReadResult {
        let capacity = usize::try_from(snapshot.len).map_err(|_| BodyReadError::StorageFailure)?;
        if snapshot.contains_disk() {
            return run_storage_io(
                self.inner.storage_operation_timeout,
                StorageIoOperation::MaterializeRead,
                move || materialize_snapshot_owned(snapshot, capacity),
            )
            .await
            .map_err(|_| BodyReadError::StorageFailure);
        }
        self.scan_snapshot(snapshot, BytesScanner::with_capacity(capacity), u64::MAX)
            .await
            .map(|result| result.value)
            .map_err(operation_to_body_error)
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
        Ok(Box::new(BodySnapshotReader::new(
            snapshot,
            self.inner.storage_operation_timeout,
        )))
    }

    pub fn is_modified(&self) -> bool {
        self.inner.mutation.read().snapshot.is_some()
    }

    /// Final length of the published mutation, read atomically with its
    /// presence under one mutation-state guard.
    pub(crate) fn modified_len(&self) -> Option<u64> {
        self.inner
            .mutation
            .read()
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.len)
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

        // Every job submitted to the blocking pool owns one scan permit for
        // its full scanner lifetime. This bounds CPU-only memory scans as well
        // as disk scans without conflating either with the per-operation
        // storage-I/O deadline lane.
        let permit = Arc::clone(scan_limiter())
            .acquire_owned()
            .await
            .map_err(|_| BodyOperationError::Body(BodyReadError::Incomplete))?;
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
        // BytesScanner must consume the complete slice. Prove its scan budget
        // before converting or reserving the output capacity, rather than
        // allocating a body-sized buffer for a deterministic limit failure.
        if slice.len > max_scan_bytes {
            return Err(BodyOperationError::ScanLimitExceeded { limit: max_scan_bytes });
        }
        let capacity = usize::try_from(slice.len).map_err(|_| BodyOperationError::Body(BodyReadError::Incomplete))?;
        self.scan_snapshot(slice, BytesScanner::with_capacity(capacity), max_scan_bytes)
            .await
    }

    pub async fn sha256_scan(&self, max_scan_bytes: u64) -> Result<BodyScanResult<[u8; 32]>, BodyOperationError> {
        let (snapshot, _) = self.snapshot().map_err(BodyOperationError::Body)?;
        let scan_snapshot = Arc::clone(&snapshot);
        let mut bytes_scanned = 0;
        let value = snapshot
            .sha256
            .get_or_try_init(|| async {
                let scan = self
                    .scan_snapshot(scan_snapshot, Sha256Scanner(Sha256::new()), max_scan_bytes)
                    .await?;
                bytes_scanned = scan.bytes_scanned;
                Ok::<[u8; 32], BodyOperationError>(scan.value)
            })
            .await?;
        Ok(BodyScanResult {
            value: *value,
            bytes_scanned,
        })
    }

    pub fn clear(&self) -> Result<(), BodyOperationError> {
        let (snapshot, generation) = self.snapshot().map_err(BodyOperationError::Body)?;
        if snapshot.len == 0 {
            return Ok(());
        }
        self.publish(generation, BodySnapshot::empty()).map(|_| ())
    }

    pub fn append(&self, bytes: Bytes) -> Result<(), BodyOperationError> {
        if bytes.is_empty() {
            return Ok(());
        }
        let (snapshot, generation) = self.snapshot().map_err(BodyOperationError::Body)?;
        if snapshot.segments.len() == MAX_BODY_SEGMENTS {
            return Err(BodyOperationError::SegmentLimitExceeded {
                limit: MAX_BODY_SEGMENTS,
            });
        }
        let mut segments = snapshot.segments.clone();
        segments.push(Segment::Inline(bytes));
        self.publish(generation, BodySnapshot::from_segments(segments)?)
            .map(|_| ())
    }

    pub fn prepend(&self, bytes: Bytes) -> Result<(), BodyOperationError> {
        if bytes.is_empty() {
            return Ok(());
        }
        let (snapshot, generation) = self.snapshot().map_err(BodyOperationError::Body)?;
        if snapshot.segments.len() == MAX_BODY_SEGMENTS {
            return Err(BodyOperationError::SegmentLimitExceeded {
                limit: MAX_BODY_SEGMENTS,
            });
        }
        let mut segments = Vec::with_capacity(snapshot.segments.len() + 1);
        segments.push(Segment::Inline(bytes));
        segments.extend(snapshot.segments.iter().cloned());
        self.publish(generation, BodySnapshot::from_segments(segments)?)
            .map(|_| ())
    }

    pub fn remove(&self, range: Range<u64>) -> Result<(), BodyOperationError> {
        let (snapshot, generation) = self.snapshot().map_err(BodyOperationError::Body)?;
        if range.start > range.end || range.end > snapshot.len {
            return Err(BodyOperationError::InvalidRange);
        }
        if range.is_empty() {
            return Ok(());
        }
        let mut segments = snapshot.slice(0..range.start)?;
        segments.extend(snapshot.slice(range.end..snapshot.len)?);
        self.publish(generation, BodySnapshot::from_segments(segments)?)
            .map(|_| ())
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
        if from == to {
            return Ok(BodyScanResult {
                value: (),
                bytes_scanned: 0,
            });
        }
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

        let mut segments = Vec::with_capacity(
            snapshot
                .segments
                .len()
                .saturating_add(scan.value.len())
                .min(MAX_BODY_SEGMENTS),
        );
        let mut splicer = SegmentSplicer::new(&snapshot);
        for range in scan.value {
            splicer.append_until(range.start, &mut segments)?;
            if !to.is_empty() {
                push_normalized_segment(&mut segments, Segment::Inline(to.clone()))?;
            }
            splicer.skip_until(range.end)?;
        }
        splicer.append_until(snapshot.len, &mut segments)?;
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
        if start == end && replacement.is_empty() {
            return self.materialize_snapshot(snapshot).await;
        }
        let mut segments = snapshot.slice(0..start).map_err(operation_to_body_error)?;
        if !replacement.is_empty() {
            segments.push(Segment::Inline(replacement));
        }
        segments.extend(snapshot.slice(end..snapshot.len).map_err(operation_to_body_error)?);
        let next = Arc::new(BodySnapshot::from_segments(segments).map_err(operation_to_body_error)?);
        if next.len > self.inner.max_body_size_bytes {
            return Err(BodyReadError::BufferLimitExceeded {
                limit: self.inner.max_body_size_bytes,
            });
        }
        let result = self.materialize_snapshot(Arc::clone(&next)).await?;
        self.publish_arc(generation, next).map_err(operation_to_body_error)?;
        Ok(result)
    }

    /// Current logical snapshot generation.
    ///
    /// A caller that computes byte offsets against one snapshot binds them to
    /// this value and passes it back to [`Self::replace_spans`], so a mutation
    /// published by anything else in between is detected instead of splicing
    /// offsets into a body they were never computed against.
    pub fn generation(&self) -> u64 {
        self.inner.mutation.read().generation
    }

    /// Apply several disjoint byte-range replacements as one atomic mutation.
    ///
    /// `edits` must be sorted by start offset and strictly non-overlapping.
    /// Segments are spliced in ascending order against the captured snapshot,
    /// so original offsets are consumed exactly once and never re-indexed after
    /// a length change — the reverse-order hazard of repeated single-range
    /// edits cannot arise. One generation compare-and-set, one segment-count
    /// check, and one inclusive physical-cap check cover the whole batch, so a
    /// partially masked body is never observable. Nothing is materialized.
    pub fn replace_spans(
        &self,
        expected_generation: u64,
        edits: &[(Range<u64>, Bytes)],
    ) -> Result<u64, BodyOperationError> {
        let (snapshot, generation) = self.snapshot().map_err(BodyOperationError::Body)?;
        if generation != expected_generation {
            return Err(BodyOperationError::Body(BodyReadError::GenerationConflict));
        }
        if edits.is_empty() {
            return Ok(generation);
        }
        if edits.len() > MAX_BODY_SEGMENTS {
            return Err(BodyOperationError::SegmentLimitExceeded {
                limit: MAX_BODY_SEGMENTS,
            });
        }

        let mut segments = Vec::with_capacity(
            snapshot
                .segments
                .len()
                .saturating_add(edits.len())
                .min(MAX_BODY_SEGMENTS),
        );
        let mut last_range_end = 0_u64;
        let mut changed = false;
        let mut splicer = SegmentSplicer::new(&snapshot);
        for (range, replacement) in edits {
            if range.start < last_range_end || range.end < range.start || range.end > snapshot.len {
                return Err(BodyOperationError::InvalidRange);
            }
            last_range_end = range.end;
            if range.is_empty() && replacement.is_empty() {
                continue;
            }
            splicer.append_until(range.start, &mut segments)?;
            if !replacement.is_empty() {
                push_normalized_segment(&mut segments, Segment::Inline(replacement.clone()))?;
            }
            splicer.skip_until(range.end)?;
            changed = true;
        }
        if !changed {
            return Ok(generation);
        }
        splicer.append_until(snapshot.len, &mut segments)?;

        let next = Arc::new(BodySnapshot::from_segments(segments)?);
        self.publish_arc(expected_generation, next)
    }

    pub fn overwrite_body(&self, body: Bytes) -> Result<(), BodyReadError> {
        let (_, generation) = self.snapshot()?;
        let snapshot = BodySnapshot::from_inline(body).map_err(operation_to_body_error)?;
        self.publish(generation, snapshot)
            .map(|_| ())
            .map_err(operation_to_body_error)
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
    storage_operation_timeout: Duration,
    retain_read_permit: bool,
}

impl BodySnapshotReader {
    fn new(snapshot: Arc<BodySnapshot>, storage_operation_timeout: Duration) -> Self {
        Self {
            snapshot,
            segment_index: 0,
            segment_offset: 0,
            storage_operation_timeout,
            retain_read_permit: false,
        }
    }

    fn memory_segment(segment: &Segment) -> Result<Option<&[u8]>, BodyReadError> {
        match segment {
            Segment::Inline(bytes) => Ok(Some(bytes.as_ref())),
            Segment::SourceRange { source, offset, len } => match source.as_ref() {
                BodySource::Disk { .. } => Ok(None),
                BodySource::Memory(bytes) => {
                    let start = usize::try_from(*offset).map_err(|_| BodyReadError::Incomplete)?;
                    let len = usize::try_from(*len).map_err(|_| BodyReadError::Incomplete)?;
                    let end = start.checked_add(len).ok_or(BodyReadError::Incomplete)?;
                    bytes.get(start..end).map(Some).ok_or(BodyReadError::Incomplete)
                }
            },
        }
    }
}

#[async_trait]
impl RequestBodySnapshotReader for BodySnapshotReader {
    fn len(&self) -> u64 {
        self.snapshot.len
    }

    fn memory_bytes_to_isolate(&self) -> Option<usize> {
        if self.segment_index != 0 || self.segment_offset != 0 {
            return None;
        }
        self.snapshot.segments.iter().try_fold(0usize, |total, segment| {
            total.checked_add(Self::memory_segment(segment).ok()?.map_or(0, <[u8]>::len))
        })
    }

    fn isolate_memory(&mut self) -> Result<(), BodyReadError> {
        self.memory_bytes_to_isolate().ok_or(BodyReadError::Incomplete)?;
        let mut segments = Vec::with_capacity(self.snapshot.segments.len());
        for segment in &self.snapshot.segments {
            segments.push(match Self::memory_segment(segment)? {
                Some(bytes) => {
                    // Exact owned payload: a small logical slice must not pin
                    // an arbitrarily large allocation from the main request.
                    Segment::Inline(Bytes::from(bytes.to_vec().into_boxed_slice()))
                }
                None => segment.clone(),
            });
        }
        self.snapshot = Arc::new(BodySnapshot::from_segments(segments).map_err(operation_to_body_error)?);
        self.retain_read_permit = true;
        Ok(())
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
                    let segment = segment.clone();
                    let segment_offset = self.segment_offset;
                    let work = move || {
                        read_segment_chunk(&segment, segment_offset, wanted)
                            .map_err(|error| io::Error::other(error.to_string()))
                    };
                    if self.retain_read_permit {
                        let output = run_storage_io_retained_with_limiter(
                            Arc::clone(storage_io_limiter()),
                            self.storage_operation_timeout,
                            StorageIoOperation::SnapshotRead,
                            work,
                        )
                        .await
                        .map_err(|_| BodyReadError::StorageFailure)?;
                        Bytes::from_owner(output)
                    } else {
                        run_storage_io(self.storage_operation_timeout, StorageIoOperation::SnapshotRead, work)
                            .await
                            .map_err(|_| BodyReadError::StorageFailure)?
                    }
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
    let mut disk_scratch = Vec::new();
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
            let control = match segment {
                Segment::SourceRange { source, offset, len } if matches!(source.as_ref(), BodySource::Disk { .. }) => {
                    let end = local_offset
                        .checked_add(wanted as u64)
                        .ok_or(BodyOperationError::Body(BodyReadError::Incomplete))?;
                    if end > *len {
                        return Err(BodyOperationError::Body(BodyReadError::Incomplete));
                    }
                    let source_offset = offset
                        .checked_add(local_offset)
                        .ok_or(BodyOperationError::Body(BodyReadError::Incomplete))?;
                    let BodySource::Disk { file, len: source_len } = source.as_ref() else {
                        unreachable!("disk segment checked above")
                    };
                    if source_offset
                        .checked_add(wanted as u64)
                        .is_none_or(|end| end > *source_len)
                    {
                        return Err(BodyOperationError::Body(BodyReadError::Incomplete));
                    }
                    disk_scratch.resize(wanted, 0);
                    read_exact_at_into(file, &mut disk_scratch, source_offset)
                        .map_err(|_| BodyOperationError::Body(BodyReadError::StorageFailure))?;
                    scanner.push(&disk_scratch)?
                }
                _ => {
                    let chunk = read_segment_chunk(segment, local_offset, wanted)?;
                    if chunk.is_empty() {
                        return Err(BodyOperationError::Body(BodyReadError::Incomplete));
                    }
                    scanner.push(&chunk)?
                }
            };
            let chunk_len = wanted as u64;
            bytes_scanned = bytes_scanned
                .checked_add(chunk_len)
                .ok_or(BodyOperationError::Body(BodyReadError::Incomplete))?;
            local_offset = local_offset
                .checked_add(chunk_len)
                .ok_or(BodyOperationError::Body(BodyReadError::Incomplete))?;
            if control == ScanControl::Stop {
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
                        .map_err(|_| BodyOperationError::Body(BodyReadError::StorageFailure))?;
                    if bytes.len() != wanted {
                        return Err(BodyOperationError::Body(BodyReadError::StorageFailure));
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
                if self.matches.len() == MAX_BODY_SEGMENTS {
                    return Err(BodyOperationError::SegmentLimitExceeded {
                        limit: MAX_BODY_SEGMENTS,
                    });
                }
                self.matches.push((self.offset - pattern_len)..self.offset);
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
    Disk { file: Arc<File> },
    FinishedMemory(Bytes),
    FinishedDisk { file: Arc<File> },
    Poisoned,
}

/// Request-body capture and replay storage supplied to Pingora's buffering seam.
pub struct BodyStorage {
    storage: CaptureStorage,
    view: Arc<BodyStorageViewInner>,
    memory_limit: usize,
    captured_len: u64,
    replay_cursor: u64,
    replay_segment_index: usize,
    replay_segment_offset: u64,
    replay_invalid: bool,
    replay_snapshot: Option<Arc<BodySnapshot>>,
    finished: bool,
    storage_operation_timeout: Duration,
    storage_failed: bool,
}

impl BodyStorage {
    pub fn new(memory_limit: usize, max_body_size_bytes: u64) -> (Self, Arc<BodyStorageView>) {
        Self::with_limits_and_timeout(memory_limit, max_body_size_bytes, DEFAULT_STORAGE_OPERATION_TIMEOUT)
    }

    pub fn with_storage_timeout(
        memory_limit: usize,
        max_body_size_bytes: u64,
        storage_operation_timeout: Duration,
    ) -> (Self, Arc<BodyStorageView>) {
        let storage_operation_timeout = if storage_operation_timeout.is_zero() {
            DEFAULT_STORAGE_OPERATION_TIMEOUT
        } else {
            storage_operation_timeout
        };
        Self::with_limits_and_timeout(memory_limit, max_body_size_bytes, storage_operation_timeout)
    }

    #[cfg(test)]
    fn with_memory_limit(memory_limit: usize) -> (Self, Arc<BodyStorageView>) {
        Self::with_limits_and_timeout(memory_limit, u64::MAX, DEFAULT_STORAGE_OPERATION_TIMEOUT)
    }

    #[cfg(test)]
    fn with_limits(memory_limit: usize, max_body_size_bytes: u64) -> (Self, Arc<BodyStorageView>) {
        Self::with_limits_and_timeout(memory_limit, max_body_size_bytes, DEFAULT_STORAGE_OPERATION_TIMEOUT)
    }

    fn with_limits_and_timeout(
        memory_limit: usize,
        max_body_size_bytes: u64,
        storage_operation_timeout: Duration,
    ) -> (Self, Arc<BodyStorageView>) {
        let view = Arc::new(BodyStorageViewInner {
            initial: OnceLock::new(),
            materialized: OnceCell::new(),
            mutation: RwLock::new(MutationState::default()),
            max_body_size_bytes,
            storage_operation_timeout,
        });
        (
            Self {
                storage: CaptureStorage::Memory(BytesMut::new()),
                view: Arc::clone(&view),
                memory_limit,
                captured_len: 0,
                replay_cursor: 0,
                replay_segment_index: 0,
                replay_segment_offset: 0,
                replay_invalid: false,
                replay_snapshot: None,
                finished: false,
                storage_operation_timeout,
                storage_failed: false,
            },
            Arc::new(BodyStorageView::new(view)),
        )
    }

    fn publish(&self, source: BodySource) -> PingoraResult<()> {
        let source = Arc::new(source);
        let snapshot = Arc::new(BodySnapshot::from_source(Arc::clone(&source)).map_err(|_| {
            PingoraError::explain(
                ErrorType::InternalError,
                "request body source length cannot be represented",
            )
        })?);
        self.view.initial.set(InitialBody { source, snapshot }).map_err(|_| {
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
        if data.is_empty() {
            return if matches!(self.storage, CaptureStorage::Poisoned) {
                Err(Self::replay_error(
                    "request body storage is unusable after a failed spill",
                ))
            } else {
                Ok(())
            };
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

        let storage = std::mem::replace(&mut self.storage, CaptureStorage::Poisoned);
        match storage {
            CaptureStorage::Memory(mut buf) if next_len <= self.memory_limit as u64 => {
                buf.extend_from_slice(data);
                self.storage = CaptureStorage::Memory(buf);
            }
            CaptureStorage::Memory(buf) => {
                let previous = buf.freeze();
                let incoming = data.clone();
                let spill = match run_storage_io(
                    self.storage_operation_timeout,
                    StorageIoOperation::SpillCreate,
                    move || create_spill_file(&previous, &incoming),
                )
                .await
                {
                    Ok(spill) => spill,
                    Err(error) => {
                        self.storage_failed = true;
                        return Err(storage_io_pingora_error("request body spill creation failed", error));
                    }
                };
                self.storage = CaptureStorage::Disk { file: spill };
            }
            CaptureStorage::Disk { file } => {
                let write_file = Arc::clone(&file);
                let offset = self.captured_len;
                let incoming = data.clone();
                if let Err(error) = run_storage_io(
                    self.storage_operation_timeout,
                    StorageIoOperation::SpillWrite,
                    move || write_all_at(&write_file, &incoming, offset),
                )
                .await
                {
                    self.storage_failed = true;
                    return Err(storage_io_pingora_error("request body spill write failed", error));
                }
                self.storage = CaptureStorage::Disk { file };
            }
            CaptureStorage::FinishedMemory(body) => {
                self.storage = CaptureStorage::FinishedMemory(body);
                return Err(Self::replay_error("request body capture continued after finalization"));
            }
            CaptureStorage::FinishedDisk { file } => {
                self.storage = CaptureStorage::FinishedDisk { file };
                return Err(Self::replay_error("request body capture continued after finalization"));
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
            CaptureStorage::Disk { file } => {
                self.publish(BodySource::Disk {
                    file: Arc::clone(&file),
                    len: self.captured_len,
                })?;
                self.storage = CaptureStorage::FinishedDisk { file };
            }
            CaptureStorage::FinishedMemory(body) => self.storage = CaptureStorage::FinishedMemory(body),
            CaptureStorage::FinishedDisk { file } => {
                self.storage = CaptureStorage::FinishedDisk { file };
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
        if self.storage_failed {
            return Err(Self::replay_error(
                "request body storage is unusable after a file operation failure",
            ));
        }
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
        self.replay_segment_index = 0;
        self.replay_segment_offset = 0;
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
        if self.storage_failed {
            return Err(Self::replay_error(
                "request body storage is unusable after a file operation failure",
            ));
        }
        let snapshot = Arc::clone(
            self.replay_snapshot
                .as_ref()
                .ok_or_else(|| Self::replay_error("request body replay requires rewind"))?,
        );
        if self.replay_cursor == snapshot.len {
            return Ok(None);
        }
        if self.replay_cursor > snapshot.len {
            return Err(Self::replay_error("request body replay cursor exceeds captured length"));
        }
        let wanted = usize::try_from((snapshot.len - self.replay_cursor).min(max_bytes as u64))
            .map_err(|_| Self::replay_error("request body replay chunk length does not fit in usize"))?;
        let segment = snapshot
            .segments
            .get(self.replay_segment_index)
            .ok_or_else(|| Self::replay_error("request body replay snapshot is inconsistent"))?;
        let segment_len = segment
            .len()
            .map_err(|_| Self::replay_error("request body replay segment length overflow"))?;
        if self.replay_segment_offset >= segment_len {
            return Err(Self::replay_error("request body replay segment cursor is inconsistent"));
        }
        let remaining = segment_len - self.replay_segment_offset;
        let wanted = wanted.min(
            usize::try_from(remaining)
                .map_err(|_| Self::replay_error("request body replay segment length overflow"))?,
        );
        let chunk = match segment {
            Segment::SourceRange { source, offset, .. } => match source.as_ref() {
                BodySource::Memory(bytes) => {
                    let start = usize::try_from(
                        offset
                            .checked_add(self.replay_segment_offset)
                            .ok_or_else(|| Self::replay_error("request body replay offset overflow"))?,
                    )
                    .map_err(|_| Self::replay_error("request body replay offset overflow"))?;
                    bytes.slice(start..start + wanted)
                }
                BodySource::Disk { file, .. } => {
                    let file = Arc::clone(file);
                    let offset = offset
                        .checked_add(self.replay_segment_offset)
                        .ok_or_else(|| Self::replay_error("request body replay offset overflow"))?;
                    let bytes = match run_storage_io(
                        self.storage_operation_timeout,
                        StorageIoOperation::ReplayRead,
                        move || read_at_most(&file, wanted, offset),
                    )
                    .await
                    {
                        Ok(bytes) => bytes,
                        Err(error) => {
                            self.storage_failed = true;
                            return Err(storage_io_pingora_error("request body replay read failed", error));
                        }
                    };
                    if bytes.len() != wanted {
                        self.storage_failed = true;
                        return Err(Self::replay_error(format!(
                            "request body replay truncated: served {} of {} captured bytes",
                            self.replay_cursor, snapshot.len
                        )));
                    }
                    Bytes::from(bytes)
                }
            },
            Segment::Inline(bytes) => {
                let start = usize::try_from(self.replay_segment_offset)
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

        let mut segment_index = self.replay_segment_index;
        let mut segment_offset = self.replay_segment_offset;
        let mut remaining = bytes;
        while remaining > 0 {
            let Some(segment) = snapshot.segments.get(segment_index) else {
                self.replay_invalid = true;
                return;
            };
            let Ok(segment_len) = segment.len() else {
                self.replay_invalid = true;
                return;
            };
            if segment_offset >= segment_len {
                self.replay_invalid = true;
                return;
            }
            let consumed = remaining.min(segment_len - segment_offset);
            segment_offset += consumed;
            remaining -= consumed;
            if segment_offset == segment_len {
                segment_index += 1;
                segment_offset = 0;
            }
        }
        self.replay_cursor = next;
        self.replay_segment_index = segment_index;
        self.replay_segment_offset = segment_offset;
    }
}

fn create_spill_file(previous: &Bytes, incoming: &Bytes) -> io::Result<Arc<File>> {
    // `tempfile()` returns an anonymous/unlinked file. If a write syscall
    // outlives its deadline, the blocking closure can retain the fd without
    // leaving a named request-body file behind.
    let file = Arc::new(tempfile()?);
    write_all_at(&file, previous, 0)?;
    write_all_at(&file, incoming, previous.len() as u64)?;
    Ok(file)
}

fn materialize_snapshot_owned(snapshot: Arc<BodySnapshot>, capacity: usize) -> io::Result<Bytes> {
    let mut body = vec![0; capacity];
    let mut output_offset = 0usize;
    for segment in &snapshot.segments {
        let segment_len = segment
            .len()
            .map_err(|_| io::Error::other("request body snapshot length is inconsistent"))?;
        let segment_len = usize::try_from(segment_len)
            .map_err(|_| io::Error::other("request body snapshot segment length overflow"))?;
        let output_end = output_offset
            .checked_add(segment_len)
            .filter(|end| *end <= body.len())
            .ok_or_else(|| io::Error::other("request body snapshot output length overflow"))?;
        copy_segment_into(segment, &mut body[output_offset..output_end])?;
        output_offset = output_end;
    }
    if output_offset != capacity {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "request body snapshot materialized to an unexpected length",
        ));
    }
    Ok(Bytes::from(body))
}

fn copy_segment_into(segment: &Segment, output: &mut [u8]) -> io::Result<()> {
    match segment {
        Segment::Inline(bytes) => {
            if bytes.len() != output.len() {
                return Err(io::Error::other("request body inline segment length is inconsistent"));
            }
            output.copy_from_slice(bytes);
        }
        Segment::SourceRange { source, offset, len } => {
            if usize::try_from(*len).ok() != Some(output.len()) {
                return Err(io::Error::other("request body source segment length is inconsistent"));
            }
            match source.as_ref() {
                BodySource::Memory(bytes) => {
                    let start = usize::try_from(*offset)
                        .map_err(|_| io::Error::other("request body source offset overflow"))?;
                    let end = start
                        .checked_add(output.len())
                        .filter(|end| *end <= bytes.len())
                        .ok_or_else(|| io::Error::other("request body source range is inconsistent"))?;
                    output.copy_from_slice(&bytes[start..end]);
                }
                BodySource::Disk { file, len: source_len } => {
                    if offset.checked_add(*len).is_none_or(|end| end > *source_len) {
                        return Err(io::Error::other("request body source range is inconsistent"));
                    }
                    read_exact_at_into(file, output, *offset)?;
                }
            }
        }
    }
    Ok(())
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

fn read_exact_at_into(file: &File, mut output: &mut [u8], mut offset: u64) -> io::Result<()> {
    while !output.is_empty() {
        let read = file.read_at(output, offset)?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "request body spill file ended before captured length",
            ));
        }
        offset = offset
            .checked_add(read as u64)
            .ok_or_else(|| io::Error::other("request body read offset overflow"))?;
        output = &mut output[read..];
    }
    Ok(())
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

async fn read_disk_body(file: Arc<File>, len: u64, storage_operation_timeout: Duration) -> BodyReadResult {
    let len = usize::try_from(len).map_err(|_| BodyReadError::StorageFailure)?;
    run_storage_io(
        storage_operation_timeout,
        StorageIoOperation::MaterializeRead,
        move || {
            let mut body = vec![0; len];
            read_exact_at_into(&file, &mut body, 0)?;
            Ok::<_, io::Error>(Bytes::from(body))
        },
    )
    .await
    .map_err(|_| BodyReadError::StorageFailure)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mirror_snapshot_read_output_holds_lane_until_last_clone() {
        let limiter = Arc::new(Semaphore::new(1));
        let worker_limiter = Arc::clone(&limiter);
        let (finished_tx, finished_rx) = tokio::sync::oneshot::channel();
        let worker = tokio::spawn(async move {
            let output = run_storage_io_retained_with_limiter(
                worker_limiter,
                Duration::from_secs(1),
                StorageIoOperation::SnapshotRead,
                || Ok(Bytes::from(vec![b'x'; SCAN_CHUNK_SIZE])),
            )
            .await
            .unwrap();
            finished_tx.send(()).unwrap();
            output
        });
        finished_rx.await.unwrap();
        // The completed output may still be waiting inside the JoinHandle.
        assert_eq!(limiter.available_permits(), 0);
        let bytes = Bytes::from_owner(worker.await.unwrap());
        let transport = bytes.slice(..1);
        drop(bytes);
        assert_eq!(limiter.available_permits(), 0);
        drop(transport);
        assert_eq!(limiter.available_permits(), 1);
    }
    use std::future::{poll_fn, Future};
    use std::task::Poll;

    struct SnapshotMemoryOwner {
        bytes: Box<[u8]>,
        dropped: Arc<AtomicBool>,
    }

    impl AsRef<[u8]> for SnapshotMemoryOwner {
        fn as_ref(&self) -> &[u8] {
            &self.bytes
        }
    }

    impl Drop for SnapshotMemoryOwner {
        fn drop(&mut self) {
            self.dropped.store(true, Ordering::Release);
        }
    }

    #[tokio::test]
    async fn snapshot_isolation_releases_large_source_and_inline_backings() {
        let source_dropped = Arc::new(AtomicBool::new(false));
        let inline_dropped = Arc::new(AtomicBool::new(false));
        let source = Bytes::from_owner(SnapshotMemoryOwner {
            bytes: vec![b'a'; 1024 * 1024].into_boxed_slice(),
            dropped: Arc::clone(&source_dropped),
        });
        let inline = Bytes::from_owner(SnapshotMemoryOwner {
            bytes: vec![b'b'; 1024 * 1024].into_boxed_slice(),
            dropped: Arc::clone(&inline_dropped),
        });
        let snapshot = BodySnapshot::from_segments(vec![
            Segment::SourceRange {
                source: Arc::new(BodySource::Memory(source)),
                offset: 500,
                len: 3,
            },
            Segment::Inline(inline.slice(700..704)),
        ])
        .unwrap();
        drop(inline);
        let mut reader = BodySnapshotReader::new(Arc::new(snapshot), DEFAULT_STORAGE_OPERATION_TIMEOUT);
        assert_eq!(reader.memory_bytes_to_isolate(), Some(7));
        assert!(!source_dropped.load(Ordering::Acquire));
        assert!(!inline_dropped.load(Ordering::Acquire));
        reader.isolate_memory().unwrap();
        assert!(source_dropped.load(Ordering::Acquire));
        assert!(inline_dropped.load(Ordering::Acquire));
        assert_eq!(reader.len(), 7);
        assert_eq!(reader.next_chunk().await.unwrap().unwrap(), "aaa");
        assert_eq!(reader.next_chunk().await.unwrap().unwrap(), "bbbb");
        assert!(reader.next_chunk().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn snapshot_isolation_preserves_disk_ranges_without_memory_charge() {
        let file = Arc::new(tempfile().unwrap());
        write_all_at(&file, b"disk payload", 0).unwrap();
        let snapshot = BodySnapshot::from_segments(vec![Segment::SourceRange {
            source: Arc::new(BodySource::Disk {
                file: Arc::clone(&file),
                len: 12,
            }),
            offset: 5,
            len: 7,
        }])
        .unwrap();
        let mut reader = BodySnapshotReader::new(Arc::new(snapshot), DEFAULT_STORAGE_OPERATION_TIMEOUT);
        assert_eq!(reader.memory_bytes_to_isolate(), Some(0));
        reader.isolate_memory().unwrap();
        assert!(reader.snapshot.contains_disk());
        assert_eq!(reader.next_chunk().await.unwrap().unwrap(), "payload");
        assert!(reader.next_chunk().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn snapshot_isolation_preserves_committed_version_and_rejects_late_isolation() {
        let (mut storage, view) = BodyStorage::new(1024, 4096);
        storage.write(&Bytes::from_static(b"original")).await.unwrap();
        storage.finish().await.unwrap();
        let mut reader = view.snapshot_reader().unwrap();
        assert_eq!(reader.memory_bytes_to_isolate(), Some(8));
        reader.isolate_memory().unwrap();
        view.overwrite_body(Bytes::from_static(b"updated")).unwrap();
        assert_eq!(reader.next_chunk().await.unwrap().unwrap(), "original");
        assert_eq!(reader.memory_bytes_to_isolate(), None);
        assert_eq!(reader.isolate_memory(), Err(BodyReadError::Incomplete));
        assert!(reader.next_chunk().await.unwrap().is_none());
    }

    #[test]
    fn snapshot_isolation_rejects_invalid_memory_range_without_panicking() {
        let snapshot = BodySnapshot::from_segments(vec![Segment::SourceRange {
            source: Arc::new(BodySource::Memory(Bytes::from_static(b"x"))),
            offset: u64::MAX,
            len: 1,
        }])
        .unwrap();
        let mut reader = BodySnapshotReader::new(Arc::new(snapshot), DEFAULT_STORAGE_OPERATION_TIMEOUT);
        assert_eq!(reader.memory_bytes_to_isolate(), None);
        assert_eq!(reader.isolate_memory(), Err(BodyReadError::Incomplete));
    }

    #[tokio::test]
    async fn disk_read_and_join_failures_are_storage_failures() {
        let empty = tempfile().expect("anonymous temp file");
        assert_eq!(
            read_disk_body(Arc::new(empty), 1, DEFAULT_STORAGE_OPERATION_TIMEOUT,).await,
            Err(BodyReadError::StorageFailure)
        );

        let join = run_storage_io(
            DEFAULT_STORAGE_OPERATION_TIMEOUT,
            StorageIoOperation::MaterializeRead,
            || -> io::Result<Bytes> { panic!("intentional join failure") },
        )
        .await;
        assert!(matches!(join, Err(StorageIoError::Join(_))));
    }

    async fn capture(storage: &mut BodyStorage, chunks: &[&[u8]]) {
        for chunk in chunks {
            storage.write(&Bytes::copy_from_slice(chunk)).await.unwrap();
        }
        storage.finish().await.unwrap();
    }

    /// `replace_spans` is the primitive the AI prompt guard masks through, so
    /// every one of its guarantees is exercised directly here rather than
    /// through a mocked session.
    mod replace_spans {
        use super::*;

        const BODY: &[u8] = br#"{"a":"one","b":"two","c":"three"}"#;

        fn edit(range: std::ops::Range<u64>, text: &str) -> (std::ops::Range<u64>, Bytes) {
            (range, Bytes::from(text.to_string()))
        }

        /// Byte range of the Nth quoted value token in BODY.
        fn token(value: &str) -> std::ops::Range<u64> {
            let quoted = format!("\"{value}\"");
            let start = std::str::from_utf8(BODY).unwrap().find(&quoted).expect("token present") as u64;
            start..start + quoted.len() as u64
        }

        async fn view() -> (BodyStorage, Arc<BodyStorageView>) {
            let (mut storage, view) = BodyStorage::new(64 * 1024, 1024 * 1024);
            capture(&mut storage, &[BODY]).await;
            (storage, view)
        }

        #[tokio::test]
        async fn several_disjoint_edits_apply_as_one_generation() {
            let (mut storage, view) = view().await;
            let before = view.generation();
            let after = view
                .replace_spans(
                    before,
                    &[edit(token("one"), "\"1\""), edit(token("three"), "\"333333\"")],
                )
                .expect("batch applies");

            // One publish for the whole batch, not one per edit.
            assert_eq!(after, before + 1);
            assert_eq!(
                replay_all(&mut storage, 4096).await,
                Bytes::from_static(br#"{"a":"1","b":"two","c":"333333"}"#)
            );
        }

        #[tokio::test]
        async fn publication_returns_its_own_generation_after_a_later_commit() {
            let (_storage, view) = view().await;
            let (snapshot, before) = view.snapshot().unwrap();

            let first = view.publish_arc(before, Arc::clone(&snapshot)).unwrap();
            let second = view.publish_arc(first, snapshot).unwrap();

            assert_eq!(first, before + 1);
            assert_eq!(second, first + 1);
            assert_eq!(view.generation(), second);
        }

        #[tokio::test]
        async fn a_stale_generation_is_a_conflict_and_publishes_nothing() {
            let (mut storage, view) = view().await;
            let stale = view.generation();
            view.replace_spans(stale, &[edit(token("one"), "\"1\"")])
                .expect("first batch applies");

            // Offsets computed against the old snapshot must never be spliced
            // into the new one.
            let error = view
                .replace_spans(stale, &[edit(token("two"), "\"2\"")])
                .expect_err("stale generation must be refused");
            assert!(matches!(
                error,
                BodyOperationError::Body(BodyReadError::GenerationConflict)
            ));
            assert_eq!(
                replay_all(&mut storage, 4096).await,
                Bytes::from_static(br#"{"a":"1","b":"two","c":"three"}"#)
            );
        }

        #[tokio::test]
        async fn overlapping_unsorted_and_out_of_range_edits_are_refused_atomically() {
            let (mut storage, view) = view().await;
            let generation = view.generation();
            let one = token("one");
            let three = token("three");

            for bad in [
                // Descending order: offsets are consumed once, ascending.
                vec![edit(three.clone(), "\"x\""), edit(one.clone(), "\"y\"")],
                // Overlapping ranges.
                vec![
                    edit(one.start..one.end, "\"x\""),
                    edit(one.start + 1..one.end + 1, "\"y\""),
                ],
                // Inverted range.
                vec![edit(one.end..one.start, "\"x\"")],
                // Past the end of the body.
                vec![edit(0..u64::from(u32::MAX), "\"x\"")],
                // A no-op still participates in ordering validation.
                vec![edit(three.end..three.end, ""), edit(one.clone(), "\"x\"")],
            ] {
                assert!(
                    view.replace_spans(generation, &bad).is_err(),
                    "malformed edit set must be refused"
                );
            }

            // Nothing was published by any of the refused batches.
            assert_eq!(view.generation(), generation);
            assert_eq!(replay_all(&mut storage, 4096).await, Bytes::from_static(BODY));
        }

        #[tokio::test]
        async fn an_empty_edit_set_publishes_nothing() {
            let (_storage, view) = view().await;
            let generation = view.generation();
            assert_eq!(view.replace_spans(generation, &[]).expect("no-op"), generation);
            assert_eq!(view.generation(), generation);
            assert!(!view.is_modified());
        }

        #[tokio::test]
        async fn growth_past_the_physical_cap_fails_and_leaves_the_body_intact() {
            let (mut storage, view) = BodyStorage::new(64 * 1024, 64);
            capture(&mut storage, &[BODY]).await;
            let generation = view.generation();

            let oversized = format!("\"{}\"", "x".repeat(64));
            let error = view
                .replace_spans(generation, &[edit(token("one"), &oversized)])
                .expect_err("growth past the cap must fail");
            assert!(matches!(
                error,
                BodyOperationError::Body(BodyReadError::BufferLimitExceeded { .. })
            ));
            assert_eq!(view.generation(), generation);
            assert_eq!(replay_all(&mut storage, 4096).await, Bytes::from_static(BODY));
        }

        #[tokio::test]
        async fn a_shrinking_mask_is_forwarded_and_reframed() {
            let (mut storage, view) = view().await;
            let generation = view.generation();
            view.replace_spans(generation, &[edit(token("three"), "\"\"")])
                .expect("shrink applies");

            let replayed = replay_all(&mut storage, 4096).await;
            assert_eq!(replayed, Bytes::from_static(br#"{"a":"one","b":"two","c":""}"#));
            // Framing consumers read the mutated length, not the original.
            assert!(view.is_modified());
            assert_eq!(view.len().unwrap(), replayed.len() as u64);
        }

        #[tokio::test]
        async fn a_mask_over_a_disk_spilled_body_is_applied_and_replayed() {
            // A tiny memory budget forces the spill path.
            let (mut storage, view) = BodyStorage::new(8, 1024 * 1024);
            capture(&mut storage, &[BODY]).await;
            let generation = view.generation();

            view.replace_spans(generation, &[edit(token("two"), "\"22\"")])
                .expect("spilled mask applies");
            assert_eq!(
                replay_all(&mut storage, 4096).await,
                Bytes::from_static(br#"{"a":"one","b":"22","c":"three"}"#)
            );
        }

        #[tokio::test]
        async fn exhausting_the_segment_budget_fails_atomically() {
            let filler = format!("{{\"k\":\"{}\"}}", "ab".repeat(4096));
            let (mut storage, view) = BodyStorage::new(1024 * 1024, 1024 * 1024);
            capture(&mut storage, &[filler.as_bytes()]).await;
            let generation = view.generation();

            // Each edit splits the body further; enough of them exhaust
            // MAX_BODY_SEGMENTS.
            let edits: Vec<_> = (0..MAX_BODY_SEGMENTS)
                .map(|i| {
                    let start = 8 + (i as u64) * 2;
                    (start..start + 1, Bytes::from_static(b"z"))
                })
                .collect();
            assert!(view.replace_spans(generation, &edits).is_err());
            assert_eq!(view.generation(), generation);
            assert_eq!(replay_all(&mut storage, 4096).await.len(), filler.len());
        }

        #[tokio::test]
        async fn many_edits_traverse_a_maximally_segmented_snapshot_monotonically() {
            let (mut storage, view) = BodyStorage::with_limits(64, u64::MAX);
            capture(&mut storage, &[b"x"]).await;
            let segmented =
                BodySnapshot::from_segments(vec![Segment::Inline(Bytes::from_static(b"x")); MAX_BODY_SEGMENTS])
                    .unwrap();
            let generation = view.publish(view.generation(), segmented).unwrap();
            let edits: Vec<_> = (0..MAX_BODY_SEGMENTS)
                .step_by(2)
                .map(|offset| (offset as u64..offset as u64 + 1, Bytes::from_static(b"z")))
                .collect();

            view.replace_spans(generation, &edits).unwrap();
            let replayed = replay_all(&mut storage, 1).await;
            assert_eq!(replayed.len(), MAX_BODY_SEGMENTS);
            assert!(replayed
                .iter()
                .enumerate()
                .all(|(index, byte)| { *byte == if index % 2 == 0 { b'z' } else { b'x' } }));
        }
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

    fn disk_file(storage: &BodyStorage) -> &Arc<File> {
        match &storage.storage {
            CaptureStorage::FinishedDisk { file } => file,
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
    async fn sha256_cache_scans_each_memory_or_disk_snapshot_once() {
        let body = b"cached request body";
        let expected: [u8; 32] = Sha256::digest(body).into();
        for memory_limit in [body.len() + 1, 1] {
            let (mut storage, view) = BodyStorage::with_memory_limit(memory_limit);
            capture(&mut storage, &[body]).await;

            let first = view.sha256_scan(body.len() as u64).await.unwrap();
            let second = view.sha256_scan(0).await.unwrap();

            assert_eq!(first.value, expected);
            assert_eq!(first.bytes_scanned, body.len() as u64);
            assert_eq!(second.value, expected);
            assert_eq!(second.bytes_scanned, 0);
            assert!(!view.is_materialized());
        }
    }

    #[tokio::test]
    async fn sha256_failed_scan_is_not_cached_and_concurrent_callers_share_success() {
        let body = vec![b'x'; SCAN_CHUNK_SIZE + 17];
        let expected: [u8; 32] = Sha256::digest(&body).into();
        let (mut storage, view) = BodyStorage::with_memory_limit(1);
        capture(&mut storage, &[&body]).await;

        assert_eq!(
            view.sha256_scan(body.len() as u64 - 1).await.unwrap_err(),
            BodyOperationError::ScanLimitExceeded {
                limit: body.len() as u64 - 1
            }
        );

        let (left, right) = tokio::join!(view.sha256_scan(body.len() as u64), view.sha256_scan(body.len() as u64));
        let left = left.unwrap();
        let right = right.unwrap();
        assert_eq!(left.value, expected);
        assert_eq!(right.value, expected);
        assert_eq!(
            left.bytes_scanned + right.bytes_scanned,
            body.len() as u64,
            "exactly one concurrent initializer must scan the snapshot"
        );
        assert_eq!(view.sha256_scan(0).await.unwrap().bytes_scanned, 0);
        assert!(!view.is_materialized());
    }

    #[tokio::test]
    async fn sha256_cache_is_generation_local_across_mutations_and_failures() {
        let (mut storage, view) = BodyStorage::with_limits(1, 16);
        capture(&mut storage, &[b"abc"]).await;

        async fn assert_new_generation(view: &BodyStorageView, body: &[u8]) {
            let expected: [u8; 32] = Sha256::digest(body).into();
            let first = view.sha256_scan(body.len() as u64).await.unwrap();
            assert_eq!(first.value, expected);
            assert_eq!(first.bytes_scanned, body.len() as u64);
            let cached = view.sha256_scan(0).await.unwrap();
            assert_eq!(cached.value, expected);
            assert_eq!(cached.bytes_scanned, 0);
        }

        assert_new_generation(&view, b"abc").await;
        assert_eq!(
            view.append(Bytes::from_static(b"body-too-large")),
            Err(BodyOperationError::Body(BodyReadError::BufferLimitExceeded {
                limit: 16
            }))
        );
        assert_eq!(view.sha256_scan(0).await.unwrap().bytes_scanned, 0);

        view.append(Bytes::from_static(b"d")).unwrap();
        assert_new_generation(&view, b"abcd").await;
        view.prepend(Bytes::from_static(b"0")).unwrap();
        assert_new_generation(&view, b"0abcd").await;
        view.remove(1..3).unwrap();
        assert_new_generation(&view, b"0cd").await;
        view.replace_literal(Bytes::from_static(b"c"), Bytes::from_static(b"XY"), 3)
            .await
            .unwrap();
        assert_new_generation(&view, b"0XYd").await;
        assert_eq!(
            view.replace_slice(1..3, Bytes::from_static(b"z")).await.unwrap(),
            Bytes::from_static(b"0zd")
        );
        assert_new_generation(&view, b"0zd").await;
        view.overwrite_body(Bytes::from_static(b"new")).unwrap();
        assert_new_generation(&view, b"new").await;
        view.clear().unwrap();
        assert_new_generation(&view, b"").await;
    }

    #[tokio::test]
    async fn digest_completed_for_old_snapshot_cannot_populate_new_generation() {
        let (mut storage, view) = BodyStorage::with_memory_limit(1);
        capture(&mut storage, &[b"old"]).await;
        let (old_snapshot, _) = view.snapshot().unwrap();

        view.overwrite_body(Bytes::from_static(b"new")).unwrap();
        let old_for_scan = Arc::clone(&old_snapshot);
        let old_digest = old_snapshot
            .sha256
            .get_or_try_init(|| async {
                view.scan_snapshot(old_for_scan, Sha256Scanner(Sha256::new()), u64::MAX)
                    .await
                    .map(|scan| scan.value)
            })
            .await
            .unwrap();
        let expected_old: [u8; 32] = Sha256::digest(b"old").into();
        assert_eq!(*old_digest, expected_old);

        let current = view.sha256_scan(u64::MAX).await.unwrap();
        let expected_new: [u8; 32] = Sha256::digest(b"new").into();
        assert_eq!(current.value, expected_new);
        assert_eq!(current.bytes_scanned, 3);
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
    async fn identity_literal_replace_does_not_scan_publish_or_materialize() {
        for memory_limit in [64, 1] {
            let (mut storage, view) = BodyStorage::with_memory_limit(memory_limit);
            capture(&mut storage, &[b"same-same"]).await;
            let generation = view.generation();

            let result = view
                .replace_literal(Bytes::from_static(b"same"), Bytes::from_static(b"same"), 0)
                .await
                .unwrap();
            assert_eq!(result.bytes_scanned, 0);
            assert_eq!(view.generation(), generation);
            assert!(!view.is_modified());
            assert!(!view.is_materialized());
            assert_eq!(replay_all(&mut storage, 2).await, Bytes::from_static(b"same-same"));
        }
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
    async fn range_budget_is_checked_before_output_capacity() {
        let source = Arc::new(BodySource::Disk {
            file: Arc::new(tempfile().unwrap()),
            len: u64::MAX,
        });
        let snapshot = Arc::new(BodySnapshot::from_source(Arc::clone(&source)).unwrap());
        let inner = Arc::new(BodyStorageViewInner {
            initial: OnceLock::new(),
            materialized: OnceCell::new(),
            mutation: RwLock::new(MutationState::default()),
            max_body_size_bytes: u64::MAX,
            storage_operation_timeout: DEFAULT_STORAGE_OPERATION_TIMEOUT,
        });
        inner.initial.set(InitialBody { source, snapshot }).ok().unwrap();
        let view = BodyStorageView::new(inner);

        assert_eq!(
            view.read_range(0, u64::MAX, 1).await,
            Err(BodyOperationError::ScanLimitExceeded { limit: 1 })
        );
    }

    #[tokio::test]
    async fn byte_no_ops_do_not_publish_a_generation() {
        let (mut empty_storage, empty_view) = BodyStorage::with_memory_limit(64);
        capture(&mut empty_storage, &[]).await;
        empty_view.clear().unwrap();
        assert_eq!(empty_view.generation(), 0);
        assert!(!empty_view.is_modified());

        let (mut storage, view) = BodyStorage::with_memory_limit(64);
        capture(&mut storage, &[b"body"]).await;
        view.remove(2..2).unwrap();
        let generation = view.generation();
        assert_eq!(
            view.replace_spans(
                generation,
                &[(0..0, Bytes::new()), (2..2, Bytes::new()), (4..4, Bytes::new()),],
            )
            .unwrap(),
            generation
        );
        assert_eq!(
            view.replace_slice(1..1, Bytes::new()).await.unwrap(),
            Bytes::from_static(b"body")
        );
        assert_eq!(view.generation(), 0);
        assert!(!view.is_modified());
        assert_eq!(replay_all(&mut storage, 2).await, Bytes::from_static(b"body"));
    }

    #[tokio::test]
    async fn oversized_edit_input_is_refused_without_publication() {
        let (mut storage, view) = BodyStorage::with_memory_limit(64);
        capture(&mut storage, &[b"body"]).await;
        let edits = vec![(0..0, Bytes::from_static(b"x")); MAX_BODY_SEGMENTS + 1];

        assert_eq!(
            view.replace_spans(view.generation(), &edits),
            Err(BodyOperationError::SegmentLimitExceeded {
                limit: MAX_BODY_SEGMENTS
            })
        );
        assert_eq!(view.generation(), 0);
        assert_eq!(replay_all(&mut storage, 2).await, Bytes::from_static(b"body"));
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
        let generation = view.generation();
        assert_eq!(
            view.append(Bytes::from_static(b"x")),
            Err(BodyOperationError::SegmentLimitExceeded {
                limit: MAX_BODY_SEGMENTS
            })
        );
        assert_eq!(
            view.prepend(Bytes::from_static(b"x")),
            Err(BodyOperationError::SegmentLimitExceeded {
                limit: MAX_BODY_SEGMENTS
            })
        );
        assert_eq!(view.generation(), generation);
        assert_eq!(view.len().unwrap(), MAX_BODY_SEGMENTS as u64);
    }

    #[tokio::test]
    async fn replay_cursor_is_segment_local_and_resets_for_retry() {
        let (mut storage, view) = BodyStorage::with_limits(64, u64::MAX);
        capture(&mut storage, &[b"x"]).await;
        let segmented =
            BodySnapshot::from_segments(vec![Segment::Inline(Bytes::from_static(b"x")); MAX_BODY_SEGMENTS]).unwrap();
        view.publish(view.generation(), segmented).unwrap();

        for _ in 0..2 {
            assert_eq!(replay_all(&mut storage, 1).await.len(), MAX_BODY_SEGMENTS);
            assert_eq!(storage.replay_segment_index, MAX_BODY_SEGMENTS);
            assert_eq!(storage.replay_segment_offset, 0);
        }
    }

    #[tokio::test]
    async fn multi_chunk_spill_materializes_directly_after_mutation() {
        let body = vec![b'd'; SCAN_CHUNK_SIZE * 2 + 17];
        let (mut storage, view) = BodyStorage::with_memory_limit(1);
        capture(&mut storage, &[&body]).await;
        view.append(Bytes::from_static(b"-tail")).unwrap();

        let materialized = view.read_all().await.unwrap();
        assert_eq!(&materialized[..body.len()], body.as_slice());
        assert_eq!(&materialized[body.len()..], b"-tail");
        assert!(!view.is_materialized());
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

    struct FinishCountingScanner {
        finishes: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl RawBodyScanner for FinishCountingScanner {
        type Output = ();

        fn requires_blocking_context(&self) -> bool {
            true
        }

        fn push(&mut self, _: &[u8]) -> Result<ScanControl, BodyOperationError> {
            unreachable!("finish-only scanner must not receive body bytes")
        }

        fn finish(self) -> Result<Self::Output, BodyOperationError> {
            self.finishes.fetch_add(1, Ordering::AcqRel);
            Ok(())
        }
    }

    #[tokio::test]
    async fn blocking_work_waits_for_scan_lane_admission() {
        let limiter = Arc::new(Semaphore::new(1));
        let held = Arc::clone(&limiter).acquire_owned().await.unwrap();
        let finishes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let scanner = FinishCountingScanner {
            finishes: Arc::clone(&finishes),
        };
        let mut future = Box::pin(run_blocking_scan_with_limiter(move || scanner.finish(), limiter));

        poll_fn(|context| match future.as_mut().poll(context) {
            Poll::Pending => Poll::Ready(()),
            Poll::Ready(_) => panic!("blocking finish bypassed scan-lane admission"),
        })
        .await;
        assert_eq!(finishes.load(Ordering::Acquire), 0);

        drop(held);
        future.await.unwrap();
        assert_eq!(finishes.load(Ordering::Acquire), 1);
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
        let file = disk_file(&storage);
        file.set_len(4).unwrap();
        assert_eq!(
            view.sha256_scan(u64::MAX).await.unwrap_err(),
            BodyOperationError::Body(BodyReadError::StorageFailure)
        );
        storage.rewind().await.unwrap();
        assert!(storage.next_chunk(64).await.is_err());
    }

    #[tokio::test]
    async fn later_chunk_truncation_is_a_storage_failure() {
        let body = vec![b'x'; SCAN_CHUNK_SIZE * 2 + 17];
        let (mut storage, view) = BodyStorage::with_memory_limit(1);
        capture(&mut storage, &[&body]).await;
        disk_file(&storage).set_len(SCAN_CHUNK_SIZE as u64 + 3).unwrap();

        assert_eq!(
            view.sha256_scan(u64::MAX).await.unwrap_err(),
            BodyOperationError::Body(BodyReadError::StorageFailure)
        );
        assert_eq!(view.read_all().await, Err(BodyReadError::StorageFailure));
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

    #[test]
    fn literal_match_limit_rejects_before_growing_past_the_bound() {
        let mut scanner = LiteralMatchScanner::new(vec![b'a']);
        let input = vec![b'a'; MAX_BODY_SEGMENTS + 1];

        assert_eq!(
            scanner.push(&input),
            Err(BodyOperationError::SegmentLimitExceeded {
                limit: MAX_BODY_SEGMENTS
            })
        );
        assert_eq!(scanner.matches.len(), MAX_BODY_SEGMENTS);
    }

    #[tokio::test]
    async fn dropping_storage_releases_its_anonymous_spill_while_snapshot_view_stays_readable() {
        let (mut storage, view) = BodyStorage::with_memory_limit(1);
        capture(&mut storage, &[b"persistent-body"]).await;
        drop(storage);
        assert_eq!(
            view.read_range(0, 64, 64).await.unwrap().value,
            Bytes::from_static(b"persistent-body")
        );
    }
}

#[cfg(test)]
#[path = "storage_io_tests.rs"]
mod storage_io_tests;
