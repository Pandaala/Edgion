use async_trait::async_trait;
use bytes::{Bytes, BytesMut};
use parking_lot::RwLock;
use pingora_core::protocols::http::RequestBodyBuffer;
use pingora_core::{Error as PingoraError, ErrorType, Result as PingoraResult};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io;
use std::os::unix::fs::FileExt;
use std::sync::{Arc, OnceLock};
use tempfile::NamedTempFile;
use tokio::sync::OnceCell;

use crate::plugins::runtime::traits::session::{BodyReadError, BodyReadResult};

pub const DEFAULT_MEMORY_BUFFER_BYTES: usize = 128 * 1024;

#[derive(Clone)]
enum BodySource {
    Memory(Bytes),
    Disk { file: Arc<File>, len: u64 },
}

struct BodyStorageViewInner {
    source: OnceLock<BodySource>,
    materialized: OnceCell<Bytes>,
    mutation: RwLock<Option<Arc<MutationBody>>>,
    max_body_size_bytes: u64,
}

#[derive(Clone)]
struct MutationBody {
    chunks: Vec<Bytes>,
    len: u64,
}

/// Shared request-body handle captured by request-stage plugins.
///
/// Capture/replay stays owned by Pingora through [`BodyStorage`]. For a
/// disk-backed body, the first plugin inspection materializes one bounded
/// `Bytes`; later readers, including response-stage consumers, clone that same
/// allocation. Plugins may replace byte ranges only before the upstream
/// request starts.
pub struct BodyStorageView {
    inner: Arc<BodyStorageViewInner>,
}

impl BodyStorageView {
    fn new(inner: Arc<BodyStorageViewInner>) -> Self {
        Self { inner }
    }

    pub async fn read_all(&self) -> BodyReadResult {
        if let Some(mutation) = self.inner.mutation.read().clone() {
            return Ok(materialize_chunks(&mutation.chunks, mutation.len)?);
        }
        let source = self.inner.source.get().ok_or(BodyReadError::Incomplete)?;
        match source {
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
        if let Some(mutation) = self.inner.mutation.read().as_ref() {
            return Ok(mutation.len);
        }
        match self.inner.source.get().ok_or(BodyReadError::Incomplete)? {
            BodySource::Memory(body) => u64::try_from(body.len()).map_err(|_| BodyReadError::Incomplete),
            BodySource::Disk { len, .. } => Ok(*len),
        }
    }

    pub fn is_modified(&self) -> bool {
        self.inner.mutation.read().is_some()
    }

    #[cfg(test)]
    pub(crate) fn is_materialized(&self) -> bool {
        self.inner.materialized.get().is_some()
    }

    pub async fn replace_slice(&self, range: std::ops::Range<usize>, replacement: Bytes) -> BodyReadResult {
        let body = self.read_all().await?;
        if range.start > range.end || range.end > body.len() {
            return Err(BodyReadError::Incomplete);
        }

        let mut chunks = Vec::with_capacity(3);
        if range.start > 0 {
            chunks.push(body.slice(..range.start));
        }
        if !replacement.is_empty() {
            chunks.push(replacement);
        }
        if range.end < body.len() {
            chunks.push(body.slice(range.end..));
        }
        let mutation = MutationBody::from_chunks(chunks)?;
        if mutation.len > self.inner.max_body_size_bytes {
            return Err(BodyReadError::BufferLimitExceeded {
                limit: self.inner.max_body_size_bytes,
            });
        }
        let result = materialize_chunks(&mutation.chunks, mutation.len)?;
        *self.inner.mutation.write() = Some(Arc::new(mutation));
        Ok(result)
    }

    pub fn overwrite_body(&self, body: Bytes) -> Result<(), BodyReadError> {
        let mutation = MutationBody::from_chunks(vec![body])?;
        if mutation.len > self.inner.max_body_size_bytes {
            return Err(BodyReadError::BufferLimitExceeded {
                limit: self.inner.max_body_size_bytes,
            });
        }
        *self.inner.mutation.write() = Some(Arc::new(mutation));
        Ok(())
    }

    pub async fn sha256(&self) -> Result<[u8; 32], BodyReadError> {
        if let Some(mutation) = self.inner.mutation.read().clone() {
            let mut hasher = Sha256::new();
            for chunk in &mutation.chunks {
                hasher.update(chunk);
            }
            return Ok(hasher.finalize().into());
        }

        match self.inner.source.get().ok_or(BodyReadError::Incomplete)? {
            BodySource::Memory(body) => Ok(Sha256::digest(body).into()),
            BodySource::Disk { file, len } => {
                let file = Arc::clone(file);
                let len = *len;
                tokio::task::spawn_blocking(move || hash_disk_body(file, len))
                    .await
                    .map_err(|_| BodyReadError::Incomplete)?
                    .map_err(|_| BodyReadError::Incomplete)
            }
        }
    }
}

impl MutationBody {
    fn from_chunks(chunks: Vec<Bytes>) -> Result<Self, BodyReadError> {
        let len = chunks.iter().try_fold(0u64, |total, chunk| {
            total
                .checked_add(u64::try_from(chunk.len()).map_err(|_| BodyReadError::Incomplete)?)
                .ok_or(BodyReadError::Incomplete)
        })?;
        Ok(Self { chunks, len })
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
    replay_mutation: Option<Arc<MutationBody>>,
    finished: bool,
}

impl BodyStorage {
    pub fn new(max_body_size_bytes: u64) -> (Self, Arc<BodyStorageView>) {
        Self::with_limits(DEFAULT_MEMORY_BUFFER_BYTES, max_body_size_bytes)
    }

    #[cfg(test)]
    fn with_memory_limit(memory_limit: usize) -> (Self, Arc<BodyStorageView>) {
        Self::with_limits(memory_limit, u64::MAX)
    }

    fn with_limits(memory_limit: usize, max_body_size_bytes: u64) -> (Self, Arc<BodyStorageView>) {
        let view = Arc::new(BodyStorageViewInner {
            source: OnceLock::new(),
            materialized: OnceCell::new(),
            mutation: RwLock::new(None),
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
                replay_mutation: None,
                finished: false,
            },
            Arc::new(BodyStorageView::new(view)),
        )
    }

    fn publish(&self, source: BodySource) -> PingoraResult<()> {
        self.view.source.set(source).map_err(|_| {
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
                    .map_err(|e| {
                        PingoraError::because(ErrorType::InternalError, "create request body spill file", e)
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
                    .map_err(|e| PingoraError::because(ErrorType::InternalError, "write request body spill file", e))?;
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
            CaptureStorage::FinishedMemory(body) => {
                self.storage = CaptureStorage::FinishedMemory(body);
            }
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
        self.replay_cursor = 0;
        self.replay_invalid = false;
        self.replay_mutation = self.view.mutation.read().clone();
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
        let replay_len = self
            .replay_mutation
            .as_ref()
            .map_or(self.captured_len, |mutation| mutation.len);
        if self.replay_cursor == replay_len {
            return Ok(None);
        }
        if self.replay_cursor > replay_len {
            return Err(Self::replay_error("request body replay cursor exceeds captured length"));
        }

        let remaining = replay_len - self.replay_cursor;
        let wanted = usize::try_from(remaining.min(max_bytes as u64))
            .map_err(|_| Self::replay_error("request body replay chunk length does not fit in usize"))?;

        if let Some(mutation) = &self.replay_mutation {
            return mutation_chunk(mutation, self.replay_cursor, wanted).map(Some);
        }

        match &self.storage {
            CaptureStorage::FinishedMemory(body) => {
                let start = usize::try_from(self.replay_cursor)
                    .map_err(|_| Self::replay_error("request body replay offset does not fit in usize"))?;
                Ok(Some(body.slice(start..start + wanted)))
            }
            CaptureStorage::FinishedDisk { file, .. } => {
                let file = Arc::clone(file);
                let offset = self.replay_cursor;
                let chunk = tokio::task::spawn_blocking(move || read_at_most(&file, wanted, offset))
                    .await
                    .map_err(|_| Self::replay_error("request body replay read task failed"))?
                    .map_err(|e| PingoraError::because(ErrorType::InternalError, "read request body spill file", e))?;
                if chunk.is_empty() {
                    return Err(Self::replay_error(format!(
                        "request body replay truncated: served {} of {} captured bytes",
                        self.replay_cursor, self.captured_len
                    )));
                }
                Ok(Some(Bytes::from(chunk)))
            }
            _ => Err(Self::replay_error(
                "request body replay source is unavailable after finalization",
            )),
        }
    }

    fn consume(&mut self, bytes: usize) {
        let Ok(bytes) = u64::try_from(bytes) else {
            self.replay_invalid = true;
            return;
        };
        let Some(next) = self.replay_cursor.checked_add(bytes) else {
            self.replay_invalid = true;
            return;
        };
        let replay_len = self
            .replay_mutation
            .as_ref()
            .map_or(self.captured_len, |mutation| mutation.len);
        if bytes == 0 || next > replay_len {
            self.replay_invalid = true;
            return;
        }
        self.replay_cursor = next;
    }
}

fn materialize_chunks(chunks: &[Bytes], len: u64) -> BodyReadResult {
    if chunks.is_empty() {
        return Ok(Bytes::new());
    }
    if chunks.len() == 1 {
        return Ok(chunks[0].clone());
    }
    let len = usize::try_from(len).map_err(|_| BodyReadError::Incomplete)?;
    let mut body = BytesMut::with_capacity(len);
    for chunk in chunks {
        body.extend_from_slice(chunk);
    }
    Ok(body.freeze())
}

fn mutation_chunk(mutation: &MutationBody, offset: u64, wanted: usize) -> PingoraResult<Bytes> {
    let mut skip =
        usize::try_from(offset).map_err(|_| BodyStorage::replay_error("request body mutation offset overflow"))?;
    for chunk in &mutation.chunks {
        if skip >= chunk.len() {
            skip -= chunk.len();
            continue;
        }
        return Ok(chunk.slice(skip..(skip + wanted.min(chunk.len() - skip))));
    }
    Err(BodyStorage::replay_error(
        "request body mutation chunks do not cover declared length",
    ))
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
    let mut buf = vec![0; len];
    let read = file.read_at(&mut buf, offset)?;
    buf.truncate(read);
    Ok(buf)
}

async fn read_disk_body(file: Arc<File>, len: u64) -> BodyReadResult {
    let len = usize::try_from(len).map_err(|_| BodyReadError::Incomplete)?;
    let body = tokio::task::spawn_blocking(move || {
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
    .map_err(|_| BodyReadError::Incomplete)?;
    Ok(body)
}

fn hash_disk_body(file: Arc<File>, len: u64) -> io::Result<[u8; 32]> {
    const CHUNK_SIZE: usize = 64 * 1024;
    let mut hasher = Sha256::new();
    let mut offset = 0u64;
    let mut buf = vec![0; CHUNK_SIZE];
    while offset < len {
        let wanted = usize::try_from((len - offset).min(CHUNK_SIZE as u64))
            .map_err(|_| io::Error::other("request body hash length overflow"))?;
        let read = file.read_at(&mut buf[..wanted], offset)?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "request body spill file ended while hashing",
            ));
        }
        hasher.update(&buf[..read]);
        offset = offset
            .checked_add(read as u64)
            .ok_or_else(|| io::Error::other("request body hash offset overflow"))?;
    }
    Ok(hasher.finalize().into())
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

    fn disk_file(storage: &BodyStorage) -> (&NamedTempFile, &Arc<File>) {
        match &storage.storage {
            CaptureStorage::FinishedDisk { temp_file, file } => (temp_file, file),
            _ => panic!("test requires a finished disk-backed body"),
        }
    }

    #[tokio::test]
    async fn memory_body_replays_and_rewinds_without_copying_chunks() {
        let (mut storage, view) = BodyStorage::with_memory_limit(16);
        capture(&mut storage, &[b"hello", b"-world"]).await;

        assert_eq!(view.read_all().await.unwrap(), Bytes::from_static(b"hello-world"));
        assert_eq!(replay_all(&mut storage, 3).await, Bytes::from_static(b"hello-world"));
        assert_eq!(replay_all(&mut storage, 4).await, Bytes::from_static(b"hello-world"));
    }

    #[tokio::test]
    async fn crossing_memory_limit_spills_and_view_materializes_once() {
        let (mut storage, view) = BodyStorage::with_memory_limit(4);
        capture(&mut storage, &[b"1234", b"56789"]).await;

        assert!(matches!(storage.storage, CaptureStorage::FinishedDisk { .. }));
        let first = view.read_all().await.unwrap();
        let second = view.read_all().await.unwrap();
        assert_eq!(first, Bytes::from_static(b"123456789"));
        assert_eq!(
            first.as_ptr(),
            second.as_ptr(),
            "view readers must share one materialization"
        );
        assert_eq!(replay_all(&mut storage, 2).await, first);
        assert_eq!(
            replay_all(&mut storage, 3).await,
            first,
            "a second upstream attempt must replay the same disk-backed body"
        );
    }

    #[tokio::test]
    async fn replace_slice_preserves_unmodified_backing_and_replays_mutation() {
        let (mut storage, view) = BodyStorage::with_memory_limit(64);
        capture(&mut storage, &[b"hello world"]).await;
        let original = view.read_all().await.unwrap();

        let replaced = view.replace_slice(6..11, Bytes::from_static(b"gateway")).await.unwrap();
        assert_eq!(replaced, Bytes::from_static(b"hello gateway"));
        assert_eq!(view.len().unwrap(), 13);
        assert!(view.is_modified());

        let mutation = view.inner.mutation.read().clone().unwrap();
        assert_eq!(mutation.chunks[0].as_ptr(), original.as_ptr());
        assert_eq!(replay_all(&mut storage, 4).await, replaced);
        assert_eq!(replay_all(&mut storage, 3).await, replaced);
    }

    #[tokio::test]
    async fn overwrite_body_replays_for_every_attempt() {
        let (mut storage, view) = BodyStorage::with_memory_limit(2);
        capture(&mut storage, &[b"original-on-disk"]).await;

        view.overwrite_body(Bytes::from_static(b"new")).unwrap();
        assert_eq!(view.read_all().await.unwrap(), Bytes::from_static(b"new"));
        assert_eq!(replay_all(&mut storage, 2).await, Bytes::from_static(b"new"));
        assert_eq!(replay_all(&mut storage, 1).await, Bytes::from_static(b"new"));
    }

    #[tokio::test]
    async fn finalized_empty_body_can_be_mutated_and_replayed() {
        let (mut storage, view) = BodyStorage::with_memory_limit(16);
        capture(&mut storage, &[]).await;
        assert_eq!(view.read_all().await.unwrap(), Bytes::new());

        view.overwrite_body(Bytes::from_static(b"created")).unwrap();
        assert_eq!(view.read_all().await.unwrap(), Bytes::from_static(b"created"));
        assert_eq!(replay_all(&mut storage, 3).await, Bytes::from_static(b"created"));
        assert_eq!(replay_all(&mut storage, 2).await, Bytes::from_static(b"created"));
    }

    #[tokio::test]
    async fn disk_backed_mutation_cap_is_atomic() {
        let (mut storage, view) = BodyStorage::with_limits(2, 6);
        capture(&mut storage, &[b"abcdef"]).await;
        assert!(matches!(storage.storage, CaptureStorage::FinishedDisk { .. }));

        view.overwrite_body(Bytes::from_static(b"first!")).unwrap();
        assert_eq!(
            view.replace_slice(1..2, Bytes::from_static(b"123456")).await,
            Err(BodyReadError::BufferLimitExceeded { limit: 6 })
        );
        assert_eq!(view.read_all().await.unwrap(), Bytes::from_static(b"first!"));
        assert!(view.is_modified());
        assert_eq!(replay_all(&mut storage, 2).await, Bytes::from_static(b"first!"));
        assert_eq!(replay_all(&mut storage, 3).await, Bytes::from_static(b"first!"));

        assert_eq!(
            view.replace_slice(1..2, Bytes::from_static(b"Z")).await.unwrap(),
            Bytes::from_static(b"fZrst!")
        );
        assert_eq!(replay_all(&mut storage, 3).await, Bytes::from_static(b"fZrst!"));
    }

    #[tokio::test]
    async fn sha256_streams_memory_and_disk_sources() {
        let expected: [u8; 32] = Sha256::digest(b"hash-me").into();

        let (mut memory, memory_view) = BodyStorage::with_memory_limit(64);
        capture(&mut memory, &[b"hash-", b"me"]).await;
        assert_eq!(memory_view.sha256().await.unwrap(), expected);

        let (mut disk, disk_view) = BodyStorage::with_memory_limit(2);
        capture(&mut disk, &[b"hash-", b"me"]).await;
        assert!(matches!(disk.storage, CaptureStorage::FinishedDisk { .. }));
        assert_eq!(disk_view.sha256().await.unwrap(), expected);
        assert!(disk_view.inner.materialized.get().is_none());

        disk_view.overwrite_body(Bytes::from_static(b"mutated")).unwrap();
        let mutated_expected: [u8; 32] = Sha256::digest(b"mutated").into();
        assert_eq!(disk_view.sha256().await.unwrap(), mutated_expected);
        assert!(disk_view.inner.materialized.get().is_none());
    }

    #[tokio::test]
    async fn finish_is_idempotent() {
        let (mut storage, view) = BodyStorage::with_memory_limit(4);
        capture(&mut storage, &[b"12345"]).await;
        storage.finish().await.unwrap();
        storage.finish().await.unwrap();
        assert_eq!(view.read_all().await.unwrap(), Bytes::from_static(b"12345"));
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
    async fn truncated_spill_fails_with_attributable_replay_error() {
        let (mut storage, _view) = BodyStorage::with_memory_limit(1);
        capture(&mut storage, &[b"abcdefghij"]).await;
        let (_, file) = disk_file(&storage);
        file.set_len(4).unwrap();

        storage.rewind().await.unwrap();
        let first = storage.next_chunk(64).await.unwrap().unwrap();
        assert_eq!(first, Bytes::from_static(b"abcd"));
        storage.consume(first.len());
        let err = storage
            .next_chunk(64)
            .await
            .expect_err("truncated backing file must fail replay");
        assert!(err.to_string().contains("body replay truncated: served 4 of 10"));
    }

    #[tokio::test]
    async fn dropping_storage_unlinks_spill_while_open_view_remains_readable() {
        let (mut storage, view) = BodyStorage::with_memory_limit(1);
        capture(&mut storage, &[b"persistent-body"]).await;
        let path = disk_file(&storage).0.path().to_path_buf();
        assert!(path.exists());

        drop(storage);
        assert!(!path.exists(), "NamedTempFile must unlink the spill on storage drop");
        assert_eq!(
            view.read_all().await.unwrap(),
            Bytes::from_static(b"persistent-body"),
            "the shared view owns an fd, not the temporary path"
        );
    }

    #[tokio::test]
    async fn dropping_storage_mid_capture_unlinks_spill() {
        let (mut storage, _view) = BodyStorage::with_memory_limit(1);
        storage
            .write(&Bytes::from_static(b"capture-in-progress"))
            .await
            .unwrap();
        let path = match &storage.storage {
            CaptureStorage::Disk { temp_file, .. } => temp_file.path().to_path_buf(),
            _ => panic!("test requires a disk-backed capture"),
        };
        assert!(path.exists());

        drop(storage);
        assert!(!path.exists(), "NamedTempFile must unlink an unfinished spill");
    }

    #[tokio::test]
    async fn invalid_consume_fails_instead_of_skipping_data() {
        let (mut storage, _view) = BodyStorage::with_memory_limit(16);
        capture(&mut storage, &[b"abc"]).await;
        storage.rewind().await.unwrap();
        storage.consume(4);
        assert!(storage.next_chunk(2).await.is_err());
    }
}
