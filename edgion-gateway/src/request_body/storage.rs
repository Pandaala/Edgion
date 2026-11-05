use async_trait::async_trait;
use bytes::Bytes;
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

use super::budget::{self, CaptureLease, MemoryBuffer, Reservation, SharedReservation};
use super::config::DEFAULT_STORAGE_OPERATION_TIMEOUT;
use super::operation::{
    BodyOperationError, BodyScanResult, BodyTextOperation, BodyTextResult, BoundedTextCollector, BoundedTextResult,
    BytesScanner, LossyTextScanner, RawBodyScanner, ReadBodyScanner, ScanControl, TextOperationScanner,
    MAX_BODY_SEGMENTS,
};
use crate::request_body::{BodyReadError, BodyReadResult, RequestBodySnapshotReader};

const SCAN_CHUNK_SIZE: usize = 64 * 1024;
const SCAN_CONCURRENCY: usize = 32;
const STORAGE_IO_CONCURRENCY: usize = 32;

fn scan_limiter() -> &'static Arc<Semaphore> {
    static LIMITER: OnceLock<Arc<Semaphore>> = OnceLock::new();
    LIMITER.get_or_init(|| Arc::new(Semaphore::new(SCAN_CONCURRENCY)))
}

pub(crate) async fn run_blocking_scan<T, F>(work: F) -> Result<T, BodyOperationError>
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
include!("../../tests/support/request_body/storage_test/86_macro_invocation.rs");

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
    if matches!(error, StorageIoError::Io(error) if matches!(io_body_error(error), BodyReadError::CapacityExceeded)) {
        return;
    }
    crate::observe::metrics::record_request_body_storage_io_failure(operation.as_metric_label(), error.metric_reason());
}

fn storage_io_pingora_error(context: &'static str, error: StorageIoError) -> Box<PingoraError> {
    match error {
        StorageIoError::Io(error) => {
            if matches!(io_body_error(&error), BodyReadError::CapacityExceeded) {
                capacity_pingora_error(BodyReadError::CapacityExceeded)
            } else {
                PingoraError::because(ErrorType::InternalError, context, error)
            }
        }
        StorageIoError::Join(error) => PingoraError::because(ErrorType::InternalError, context, error),
        StorageIoError::AdmissionTimeout | StorageIoError::AdmissionClosed | StorageIoError::ExecutionTimeout => {
            PingoraError::explain(ErrorType::InternalError, context)
        }
    }
}

/// File closes before its reservations are returned. Late blocking jobs own an Arc.
struct BudgetedFile {
    file: File,
    disk: SharedReservation,
    _file_slot: Reservation,
    lease: Arc<CaptureLease>,
}

impl std::ops::Deref for BudgetedFile {
    type Target = File;
    fn deref(&self) -> &File {
        &self.file
    }
}

fn capacity_pingora_error(error: BodyReadError) -> Box<PingoraError> {
    PingoraError::because(ErrorType::HTTPStatus(503), "request body capacity exhausted", error).into_in()
}

fn body_io_error(error: BodyReadError) -> io::Error {
    io::Error::other(error)
}

fn io_body_error(error: &io::Error) -> BodyReadError {
    error
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<BodyReadError>())
        .cloned()
        .unwrap_or(BodyReadError::StorageFailure)
}

fn storage_body_error(error: StorageIoError) -> BodyReadError {
    match error {
        StorageIoError::Io(error) => io_body_error(&error),
        _ => BodyReadError::StorageFailure,
    }
}

#[derive(Clone)]
enum BodySource {
    Memory(Bytes),
    Disk { file: Arc<BudgetedFile>, len: u64 },
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
    frozen: bool,
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
    lease: Result<Arc<CaptureLease>, BodyReadError>,
}

impl BodyStorageViewInner {
    fn lease(&self) -> Result<Arc<CaptureLease>, BodyReadError> {
        self.lease.clone()
    }
    fn snapshot(&self) -> Result<(Arc<BodySnapshot>, u64), BodyReadError> {
        self.lease()?;
        let mutation = self.mutation.read();
        if let Some(snapshot) = mutation.snapshot.as_ref() {
            return Ok((Arc::clone(snapshot), mutation.generation));
        }
        let initial = self.initial.get().ok_or(BodyReadError::Incomplete)?;
        Ok((Arc::clone(&initial.snapshot), mutation.generation))
    }
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
    /// Pin the inspected logical representation. Reads and replay remain
    /// available, but no subsequent mutation may invalidate an admission.
    pub(crate) fn freeze_mutation(&self) {
        self.inner.mutation.write().frozen = true;
    }

    fn new(inner: Arc<BodyStorageViewInner>) -> Self {
        Self { inner }
    }

    fn snapshot(&self) -> Result<(Arc<BodySnapshot>, u64), BodyReadError> {
        self.inner.snapshot()
    }

    fn snapshot_metadata(&self) -> Result<(u64, u64), BodyReadError> {
        self.inner.lease()?;
        let mutation = self.inner.mutation.read();
        if let Some(snapshot) = mutation.snapshot.as_ref() {
            return Ok((snapshot.len, mutation.generation));
        }
        let initial = self.inner.initial.get().ok_or(BodyReadError::Incomplete)?;
        Ok((initial.snapshot.len, mutation.generation))
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
        if mutation.frozen {
            return Err(BodyReadError::NotBuffered.into());
        }
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
        self.inner.lease()?;
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
            let buffer = MemoryBuffer::new(capacity, self.inner.lease()?)?;
            return run_storage_io(
                self.inner.storage_operation_timeout,
                StorageIoOperation::MaterializeRead,
                move || materialize_snapshot_owned(snapshot, buffer),
            )
            .await
            .map_err(storage_body_error);
        }
        self.scan_snapshot(
            snapshot,
            BytesScanner::with_capacity(capacity, self.inner.lease()?)?,
            u64::MAX,
        )
        .await
        .map(|result| result.value)
        .map_err(operation_to_body_error)
    }

    pub fn len(&self) -> Result<u64, BodyReadError> {
        self.snapshot_metadata().map(|(len, _)| len)
    }

    pub fn is_empty(&self) -> Result<bool, BodyReadError> {
        self.len().map(|len| len == 0)
    }

    /// Capture an owned reader over the current immutable logical snapshot.
    ///
    /// Creating the reader does not materialize a disk-backed source. Later
    /// mutations publish a different snapshot and cannot affect this reader.
    pub fn snapshot_reader(&self) -> Result<Box<dyn RequestBodySnapshotReader>, BodyReadError> {
        let (snapshot, _) = self.snapshot()?;
        Ok(Box::new(BodySnapshotReader::with_lease(
            snapshot,
            self.inner.storage_operation_timeout,
            self.inner.lease()?,
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
        if !scanner.requires_blocking_context() && snapshot.len <= SCAN_CHUNK_SIZE as u64 && !snapshot.contains_disk() {
            return scan_snapshot_owned(snapshot, scanner, max_scan_bytes, None);
        }

        run_cancellable_scan(move |cancelled| scan_snapshot_owned(snapshot, scanner, max_scan_bytes, Some(cancelled)))
            .await
    }

    /// Run an owned pull-based scanner over the current immutable snapshot in
    /// one admitted blocking-pool job.
    pub async fn scan_read<S>(
        &self,
        scanner: S,
        max_scan_bytes: u64,
    ) -> Result<BodyScanResult<S::Output>, BodyOperationError>
    where
        S: ReadBodyScanner,
    {
        let (snapshot, _) = self.snapshot().map_err(BodyOperationError::Body)?;
        run_cancellable_scan(move |cancelled| {
            scan_snapshot_read_owned(snapshot, scanner, max_scan_bytes, Some(cancelled))
        })
        .await
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
        let slice = self.range_snapshot(offset, max_bytes, max_scan_bytes)?;
        let capacity = usize::try_from(slice.len).map_err(|_| BodyOperationError::Body(BodyReadError::Incomplete))?;
        self.scan_snapshot(
            slice,
            BytesScanner::with_capacity(capacity, self.inner.lease()?)?,
            max_scan_bytes,
        )
        .await
    }

    /// Decode an isolated raw byte range without collecting it first. Both
    /// the returned text and requested text capacity are bounded by the output
    /// limit; overflow retains the scan's fetched-byte accounting.
    pub async fn read_text_range(
        &self,
        offset: u64,
        max_bytes: u64,
        max_output_bytes: usize,
        max_scan_bytes: u64,
    ) -> Result<BodyScanResult<BoundedTextResult>, BodyOperationError> {
        let slice = self.range_snapshot(offset, max_bytes, max_scan_bytes)?;
        self.scan_snapshot(
            slice,
            LossyTextScanner::new(BoundedTextCollector::new(max_output_bytes)),
            max_scan_bytes,
        )
        .await
    }

    fn range_snapshot(
        &self,
        offset: u64,
        max_bytes: u64,
        max_scan_bytes: u64,
    ) -> Result<Arc<BodySnapshot>, BodyOperationError> {
        let (snapshot, _) = self.snapshot().map_err(BodyOperationError::Body)?;
        if offset > snapshot.len {
            return Err(BodyOperationError::InvalidRange);
        }
        let end = offset.saturating_add(max_bytes).min(snapshot.len);
        let slice = Arc::new(BodySnapshot::from_segments(snapshot.slice(offset..end)?)?);
        // Range reads preflight the complete raw slice, preserving scan-limit
        // precedence even when bounded text collection could stop early.
        if slice.len > max_scan_bytes {
            return Err(BodyOperationError::ScanLimitExceeded { limit: max_scan_bytes });
        }
        Ok(slice)
    }

    pub async fn sha256_scan(&self, max_scan_bytes: u64) -> Result<BodyScanResult<[u8; 32]>, BodyOperationError> {
        let (snapshot, _) = self.snapshot().map_err(BodyOperationError::Body)?;
        let mut bytes_scanned = 0;
        let value = snapshot
            .sha256
            .get_or_try_init(|| async {
                let scan_snapshot = Arc::clone(&snapshot);
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

    fn check_mutation_len(&self, len: Option<u64>) -> Result<(), BodyReadError> {
        if len.is_none_or(|len| len > self.inner.max_body_size_bytes) {
            return Err(BodyReadError::BufferLimitExceeded {
                limit: self.inner.max_body_size_bytes,
            });
        }
        Ok(())
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
        self.check_mutation_len(snapshot.len.checked_add(bytes.len() as u64))?;
        let mut segments = Vec::with_capacity(snapshot.segments.len() + 1);
        segments.extend(snapshot.segments.iter().cloned());
        segments.push(Segment::Inline(MemoryBuffer::copy(&bytes, self.inner.lease()?)?));
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
        self.check_mutation_len(snapshot.len.checked_add(bytes.len() as u64))?;
        let mut segments = Vec::with_capacity(snapshot.segments.len() + 1);
        segments.push(Segment::Inline(MemoryBuffer::copy(&bytes, self.inner.lease()?)?));
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
        let from_len = from.len() as u64;
        let to_len = to.len() as u64;
        let scanner = LiteralMatchScanner::new(from.to_vec());
        drop(from);
        let admitted_to = MemoryBuffer::copy(&to, self.inner.lease()?)?;
        drop(to);
        let scan = self
            .scan_snapshot(Arc::clone(&snapshot), scanner, max_scan_bytes)
            .await?;
        if scan.value.is_empty() {
            return Ok(BodyScanResult {
                value: (),
                bytes_scanned: scan.bytes_scanned,
            });
        }

        let match_count =
            u64::try_from(scan.value.len()).map_err(|_| BodyOperationError::Body(BodyReadError::Incomplete))?;
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

        let to = admitted_to;
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
            drop(replacement);
            return self.materialize_snapshot(snapshot).await;
        }
        self.check_mutation_len(
            snapshot
                .len
                .checked_sub(end - start)
                .and_then(|len| len.checked_add(replacement.len() as u64)),
        )?;
        let mut segments = snapshot.slice(0..start).map_err(operation_to_body_error)?;
        if !replacement.is_empty() {
            segments.push(Segment::Inline(MemoryBuffer::copy(&replacement, self.inner.lease()?)?));
        }
        drop(replacement);
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

        let mut final_len = Some(snapshot.len);
        let mut previous_end = 0;
        for (range, replacement) in edits {
            if range.start < previous_end || range.end < range.start || range.end > snapshot.len {
                return Err(BodyOperationError::InvalidRange);
            }
            previous_end = range.end;
            final_len = final_len
                .and_then(|len| len.checked_sub(range.end - range.start))
                .and_then(|len| len.checked_add(replacement.len() as u64));
        }
        self.check_mutation_len(final_len)?;

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
                push_normalized_segment(
                    &mut segments,
                    Segment::Inline(MemoryBuffer::copy(replacement, self.inner.lease()?)?),
                )?;
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
        let (_, generation) = self.snapshot_metadata()?;
        if body.len() as u64 > self.inner.max_body_size_bytes {
            return Err(BodyReadError::BufferLimitExceeded {
                limit: self.inner.max_body_size_bytes,
            });
        }
        let body = MemoryBuffer::copy(&body, self.inner.lease()?)?;
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
    lease: Arc<CaptureLease>,
}

impl BodySnapshotReader {
    fn with_lease(snapshot: Arc<BodySnapshot>, storage_operation_timeout: Duration, lease: Arc<CaptureLease>) -> Self {
        Self {
            lease,
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

    fn try_take_contiguous_memory(&mut self, max_bytes: usize) -> Result<Option<Bytes>, BodyReadError> {
        if self.segment_index != 0 || self.segment_offset != 0 || self.snapshot.len == 0 {
            return Ok(None);
        }
        let [segment] = self.snapshot.segments.as_slice() else {
            return Ok(None);
        };
        let Some(memory) = Self::memory_segment(segment)? else {
            return Ok(None);
        };
        if u64::try_from(memory.len()).map_err(|_| BodyReadError::Incomplete)? != self.snapshot.len {
            return Err(BodyReadError::Incomplete);
        }
        if memory.len() > max_bytes {
            return Ok(None);
        }
        // The checked range above makes this a shared slice of the same owner.
        let bytes = read_segment_chunk(segment, 0, memory.len()).map_err(operation_to_body_error)?;
        self.segment_index = 1;
        Ok(Some(bytes))
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
                    Segment::Inline(MemoryBuffer::copy(bytes, Arc::clone(&self.lease))?)
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
                        read_segment_chunk(&segment, segment_offset, wanted).map_err(|error| match error {
                            BodyOperationError::Body(error) => body_io_error(error),
                            _ => io::Error::other(error.to_string()),
                        })
                    };
                    if self.retain_read_permit {
                        let output = run_storage_io_retained_with_limiter(
                            Arc::clone(storage_io_limiter()),
                            self.storage_operation_timeout,
                            StorageIoOperation::SnapshotRead,
                            work,
                        )
                        .await
                        .map_err(storage_body_error)?;
                        Bytes::from_owner(output)
                    } else {
                        run_storage_io(self.storage_operation_timeout, StorageIoOperation::SnapshotRead, work)
                            .await
                            .map_err(storage_body_error)?
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

/// Every job submitted to the blocking pool owns one scan permit for its full
/// scanner lifetime. This bounds CPU-only memory scans as well as disk scans
/// without conflating either with the per-operation storage-I/O deadline lane.
/// Dropping the returned future cancels the job between chunks.
async fn run_cancellable_scan<T, F>(work: F) -> Result<T, BodyOperationError>
where
    T: Send + 'static,
    F: FnOnce(&AtomicBool) -> Result<T, BodyOperationError> + Send + 'static,
{
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
        work(&task_cancelled)
    })
    .await
    .map_err(|_| BodyOperationError::Body(BodyReadError::Incomplete))?;
    guard.completed = true;
    result
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
    let mut chunks = SnapshotChunks::new(&snapshot, max_scan_bytes, cancelled);
    while chunks.advance()? {
        if scanner.push(chunks.chunk())? == ScanControl::Stop {
            break;
        }
    }
    Ok(BodyScanResult {
        value: scanner.finish()?,
        bytes_scanned: chunks.bytes_scanned,
    })
}

fn scan_snapshot_read_owned<S>(
    snapshot: Arc<BodySnapshot>,
    scanner: S,
    max_scan_bytes: u64,
    cancelled: Option<&AtomicBool>,
) -> Result<BodyScanResult<S::Output>, BodyOperationError>
where
    S: ReadBodyScanner,
{
    let mut reader = SnapshotRead {
        chunks: SnapshotChunks::new(&snapshot, max_scan_bytes, cancelled),
        position: 0,
        error: None,
    };
    let value = scanner.scan(&mut reader);
    // A parser sees a chunk failure only as an opaque I/O error; the retained
    // body error keeps budget, cancellation and storage policy intact.
    if let Some(error) = reader.error {
        return Err(error);
    }
    Ok(BodyScanResult {
        value,
        bytes_scanned: reader.chunks.bytes_scanned,
    })
}

enum CurrentChunk<'a> {
    None,
    Memory(&'a [u8]),
    Disk,
}

/// Bounded chunk cursor over one immutable snapshot. Memory segments are
/// borrowed and disk segments are read into one reusable budgeted scratch
/// buffer. Cancellation and the scan budget are checked before every fetch,
/// and each fetched chunk is charged before its consumer sees it.
struct SnapshotChunks<'a> {
    snapshot: &'a BodySnapshot,
    max_scan_bytes: u64,
    cancelled: Option<&'a AtomicBool>,
    segment_index: usize,
    local_offset: u64,
    bytes_scanned: u64,
    disk_scratch: Option<MemoryBuffer>,
    current: CurrentChunk<'a>,
}

impl<'a> SnapshotChunks<'a> {
    fn new(snapshot: &'a BodySnapshot, max_scan_bytes: u64, cancelled: Option<&'a AtomicBool>) -> Self {
        Self {
            snapshot,
            max_scan_bytes,
            cancelled,
            segment_index: 0,
            local_offset: 0,
            bytes_scanned: 0,
            disk_scratch: None,
            current: CurrentChunk::None,
        }
    }

    /// The most recently fetched chunk, or an empty slice before the first
    /// fetch and after the end of the snapshot.
    #[inline]
    fn chunk(&self) -> &[u8] {
        match self.current {
            CurrentChunk::None => &[],
            CurrentChunk::Memory(chunk) => chunk,
            CurrentChunk::Disk => self.disk_scratch.as_ref().map_or(&[], MemoryBuffer::as_slice),
        }
    }

    /// Fetch the next chunk. Returns `false` at the end of the snapshot.
    fn advance(&mut self) -> Result<bool, BodyOperationError> {
        self.current = CurrentChunk::None;
        let snapshot = self.snapshot;
        loop {
            let Some(segment) = snapshot.segments.get(self.segment_index) else {
                return Ok(false);
            };
            let segment_len = segment.len()?;
            if self.local_offset >= segment_len {
                self.segment_index += 1;
                self.local_offset = 0;
                continue;
            }
            if self.cancelled.is_some_and(|flag| flag.load(Ordering::Acquire)) {
                return Err(BodyOperationError::Body(BodyReadError::Incomplete));
            }
            if self.bytes_scanned == self.max_scan_bytes {
                return Err(BodyOperationError::ScanLimitExceeded {
                    limit: self.max_scan_bytes,
                });
            }
            let remaining_budget = self.max_scan_bytes - self.bytes_scanned;
            let local_offset = self.local_offset;
            let wanted = usize::try_from(
                (segment_len - local_offset)
                    .min(SCAN_CHUNK_SIZE as u64)
                    .min(remaining_budget),
            )
            .map_err(|_| BodyOperationError::Body(BodyReadError::Incomplete))?;
            self.current = match segment {
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
                    if self
                        .disk_scratch
                        .as_ref()
                        .is_none_or(|buffer| buffer.capacity() < wanted)
                    {
                        self.disk_scratch = Some(MemoryBuffer::new(wanted, Arc::clone(&file.lease))?);
                    }
                    let scratch = self.disk_scratch.as_mut().ok_or(BodyReadError::Incomplete)?;
                    read_exact_at_into(file, &mut scratch.writable()[..wanted], source_offset)
                        .map_err(|error| BodyOperationError::Body(io_body_error(&error)))?;
                    scratch.set_len(wanted);
                    CurrentChunk::Disk
                }
                _ => {
                    let memory = BodySnapshotReader::memory_segment(segment)
                        .map_err(BodyOperationError::Body)?
                        .ok_or(BodyOperationError::Body(BodyReadError::Incomplete))?;
                    let start = usize::try_from(local_offset)
                        .map_err(|_| BodyOperationError::Body(BodyReadError::Incomplete))?;
                    let end = start
                        .checked_add(wanted)
                        .ok_or(BodyOperationError::Body(BodyReadError::Incomplete))?;
                    let chunk = memory
                        .get(start..end)
                        .ok_or(BodyOperationError::Body(BodyReadError::Incomplete))?;
                    if chunk.is_empty() {
                        return Err(BodyOperationError::Body(BodyReadError::Incomplete));
                    }
                    CurrentChunk::Memory(chunk)
                }
            };
            let chunk_len = wanted as u64;
            self.bytes_scanned = self
                .bytes_scanned
                .checked_add(chunk_len)
                .ok_or(BodyOperationError::Body(BodyReadError::Incomplete))?;
            self.local_offset = local_offset
                .checked_add(chunk_len)
                .ok_or(BodyOperationError::Body(BodyReadError::Incomplete))?;
            return Ok(true);
        }
    }
}

/// `std::io::Read` over [`SnapshotChunks`]. The first chunk error is retained
/// and reported on every later read.
struct SnapshotRead<'a> {
    chunks: SnapshotChunks<'a>,
    position: usize,
    error: Option<BodyOperationError>,
}

impl io::Read for SnapshotRead<'_> {
    fn read(&mut self, target: &mut [u8]) -> io::Result<usize> {
        if target.is_empty() {
            return Ok(0);
        }
        loop {
            if let Some(error) = &self.error {
                return Err(io::Error::other(error.clone()));
            }
            let chunk = self.chunks.chunk();
            if let Some(mut rest) = chunk.get(self.position..).filter(|rest| !rest.is_empty()) {
                // serde_json pulls one byte per call; the slice reader keeps
                // std's single-byte fast path.
                let read = io::Read::read(&mut rest, target)?;
                self.position += read;
                return Ok(read);
            }
            match self.chunks.advance() {
                Ok(true) => self.position = 0,
                Ok(false) => return Ok(0),
                Err(error) => self.error = Some(error),
            }
        }
    }
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
                        .map_err(|error| BodyOperationError::Body(io_body_error(&error)))?;
                    if bytes.len() != wanted {
                        return Err(BodyOperationError::Body(BodyReadError::StorageFailure));
                    }
                    Ok(bytes)
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
    Memory(MemoryBuffer),
    Disk { file: Arc<BudgetedFile> },
    Finished,
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
        let lease = budget::process_budget().capture();
        let storage = match &lease {
            Ok(lease) => MemoryBuffer::new(0, Arc::clone(lease))
                .map(CaptureStorage::Memory)
                .unwrap_or(CaptureStorage::Poisoned),
            Err(_) => CaptureStorage::Poisoned,
        };
        let view = Arc::new(BodyStorageViewInner {
            lease,
            initial: OnceLock::new(),
            materialized: OnceCell::new(),
            mutation: RwLock::new(MutationState::default()),
            max_body_size_bytes,
            storage_operation_timeout,
        });
        (
            Self {
                storage,
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
        let lease = self.view.lease().map_err(capacity_pingora_error)?;
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
                let needed = next_len as usize;
                if needed > buf.capacity() {
                    let capacity = needed.max(buf.capacity().saturating_mul(2)).min(self.memory_limit);
                    buf.grow(capacity).map_err(capacity_pingora_error)?;
                }
                buf.append(data);
                self.storage = CaptureStorage::Memory(buf);
            }
            CaptureStorage::Memory(buf) => {
                let previous = buf.freeze();
                let incoming = MemoryBuffer::copy(data, Arc::clone(&lease)).map_err(capacity_pingora_error)?;
                let disk = lease.budget.disk.reserve(next_len).map_err(capacity_pingora_error)?;
                let file_slot = lease.budget.files.reserve(1).map_err(capacity_pingora_error)?;
                let spill = match run_storage_io(
                    self.storage_operation_timeout,
                    StorageIoOperation::SpillCreate,
                    move || create_spill_file(&previous, &incoming, disk, file_slot, lease),
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
                let incoming = MemoryBuffer::copy(data, lease).map_err(capacity_pingora_error)?;
                // The file owns the entire attempted growth even if a syscall
                // writes only a prefix or finishes after cancellation.
                file.disk.grow(data_len).map_err(capacity_pingora_error)?;
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
            CaptureStorage::Finished => {
                self.storage = CaptureStorage::Finished;
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
        self.view.lease().map_err(capacity_pingora_error)?;
        if self.finished {
            return Ok(());
        }
        let storage = std::mem::replace(&mut self.storage, CaptureStorage::Poisoned);
        match storage {
            CaptureStorage::Memory(buf) => {
                self.publish(BodySource::Memory(buf.freeze()))?;
                self.storage = CaptureStorage::Finished;
            }
            CaptureStorage::Disk { file } => {
                self.publish(BodySource::Disk {
                    file,
                    len: self.captured_len,
                })?;
                self.storage = CaptureStorage::Finished;
            }
            CaptureStorage::Finished => self.storage = CaptureStorage::Finished,
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
        self.replay_snapshot = Some(
            self.view
                .snapshot()
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
                    bytes
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

fn create_spill_file(
    previous: &Bytes,
    incoming: &Bytes,
    disk: Reservation,
    file_slot: Reservation,
    lease: Arc<CaptureLease>,
) -> io::Result<Arc<BudgetedFile>> {
    let file = Arc::new(BudgetedFile {
        file: tempfile()?,
        disk: disk.shared(),
        _file_slot: file_slot,
        lease,
    });
    write_all_at(&file, previous, 0)?;
    write_all_at(&file, incoming, previous.len() as u64)?;
    Ok(file)
}

fn materialize_snapshot_owned(snapshot: Arc<BodySnapshot>, mut body: MemoryBuffer) -> io::Result<Bytes> {
    let capacity = body.capacity();
    let mut output_offset = 0usize;
    for segment in &snapshot.segments {
        let segment_len = segment
            .len()
            .map_err(|_| io::Error::other("request body snapshot length is inconsistent"))?;
        let segment_len = usize::try_from(segment_len)
            .map_err(|_| io::Error::other("request body snapshot segment length overflow"))?;
        let output_end = output_offset
            .checked_add(segment_len)
            .filter(|end| *end <= body.capacity())
            .ok_or_else(|| io::Error::other("request body snapshot output length overflow"))?;
        copy_segment_into(segment, &mut body.writable()[output_offset..output_end])?;
        output_offset = output_end;
    }
    if output_offset != capacity {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "request body snapshot materialized to an unexpected length",
        ));
    }
    body.set_len(capacity);
    Ok(body.freeze())
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

fn read_at_most(file: &BudgetedFile, len: usize, offset: u64) -> io::Result<Bytes> {
    let mut buffer = MemoryBuffer::new(len, Arc::clone(&file.lease)).map_err(body_io_error)?;
    let filled = read_at_most_into(buffer.writable(), offset, |buf, position| file.read_at(buf, position))?;
    buffer.set_len(filled);
    Ok(buffer.freeze())
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

fn read_at_most_into(
    buf: &mut [u8],
    offset: u64,
    mut read_at: impl FnMut(&mut [u8], u64) -> io::Result<usize>,
) -> io::Result<usize> {
    let len = buf.len();
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
    Ok(filled)
}

async fn read_disk_body(file: Arc<BudgetedFile>, len: u64, storage_operation_timeout: Duration) -> BodyReadResult {
    let len = usize::try_from(len).map_err(|_| BodyReadError::StorageFailure)?;
    let mut body = MemoryBuffer::new(len, Arc::clone(&file.lease))?;
    run_storage_io(
        storage_operation_timeout,
        StorageIoOperation::MaterializeRead,
        move || {
            read_exact_at_into(&file, body.writable(), 0)?;
            body.set_len(len);
            Ok(body.freeze())
        },
    )
    .await
    .map_err(storage_body_error)
}

#[cfg(test)]
#[path = "../../tests/unit/request_body/storage_test.rs"]
mod tests;

#[cfg(test)]
#[path = "../../tests/unit/request_body/storage_test/storage_io_tests.rs"]
mod storage_io_tests;

#[cfg(test)]
#[path = "../../tests/unit/request_body/storage_test/budget_tests.rs"]
mod budget_tests;
