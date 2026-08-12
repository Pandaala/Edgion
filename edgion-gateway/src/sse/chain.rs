use std::borrow::Cow;

use bytes::{Bytes, BytesMut};

pub(super) const MAX_SLICE_HANDLES: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ByteRange {
    pub start: usize,
    pub end: usize,
}

impl ByteRange {
    pub const fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }

    pub fn len(self) -> usize {
        self.end.saturating_sub(self.start)
    }

    pub fn is_empty(self) -> bool {
        self.start == self.end
    }
}

#[derive(Debug, Default)]
pub(super) struct SliceChain {
    storage: ChainStorage,
    slice_store: Vec<Bytes>,
    len: usize,
    coalesce_count: usize,
    copied_bytes: usize,
}

#[derive(Debug, Default)]
enum ChainStorage {
    #[default]
    Slices,
    Coalesced(BytesMut),
}

impl SliceChain {
    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub(super) fn handle_count(&self) -> usize {
        match &self.storage {
            ChainStorage::Slices => self.slices().len(),
            ChainStorage::Coalesced(bytes) => usize::from(!bytes.is_empty()),
        }
    }

    pub(super) fn resident_capacity(&self) -> usize {
        match &self.storage {
            ChainStorage::Slices => self.slices().iter().map(Bytes::len).sum(),
            ChainStorage::Coalesced(bytes) => bytes.capacity(),
        }
    }

    pub(super) fn slice_metadata_capacity(&self) -> usize {
        match self.storage {
            ChainStorage::Slices => self.slice_store.capacity().saturating_mul(std::mem::size_of::<Bytes>()),
            ChainStorage::Coalesced(_) => 0,
        }
    }

    pub fn push(&mut self, bytes: Bytes, byte_limit: usize) -> Result<(), ()> {
        if bytes.is_empty() {
            return Ok(());
        }
        let new_len = self.len.checked_add(bytes.len()).ok_or(())?;
        if new_len > byte_limit {
            return Err(());
        }

        let slice_count = self.slice_store.len();
        match &mut self.storage {
            ChainStorage::Slices if slice_count < MAX_SLICE_HANDLES => {
                self.slices_mut().push(bytes);
            }
            ChainStorage::Slices => {
                let mut arena = BytesMut::with_capacity(byte_limit);
                if arena.capacity() > byte_limit {
                    return Err(());
                }
                for slice in self.slices() {
                    arena.extend_from_slice(slice);
                }
                arena.extend_from_slice(&bytes);
                self.copied_bytes = self.copied_bytes.saturating_add(new_len);
                self.coalesce_count = self.coalesce_count.saturating_add(1);
                self.slice_store.clear();
                self.storage = ChainStorage::Coalesced(arena);
            }
            ChainStorage::Coalesced(current) => {
                if new_len > current.capacity() {
                    return Err(());
                }
                current.extend_from_slice(&bytes);
                self.copied_bytes = self.copied_bytes.saturating_add(bytes.len());
            }
        }
        self.len = new_len;
        Ok(())
    }

    /// Normalize only shared or slack-bearing retained handles. Handles that
    /// were normalized by an earlier callback remain unique and are reused.
    pub fn normalize_retained(&mut self, byte_limit: usize) -> Result<(), ()> {
        if !matches!(self.storage, ChainStorage::Slices) {
            return Ok(());
        }
        let source = std::mem::take(&mut self.slice_store);
        let mut pending = source.into_iter();
        let mut normalized = Vec::with_capacity(pending.len());
        let mut resident = 0usize;
        while let Some(original) = pending.next() {
            let result = match original.try_into_mut() {
                Ok(bytes) if bytes.capacity() == bytes.len() => bytes.freeze(),
                Ok(bytes) => match copy_exact(&bytes, byte_limit) {
                    Ok(copy) => copy,
                    Err(()) => {
                        normalized.push(bytes.freeze());
                        normalized.extend(pending);
                        self.slice_store = normalized;
                        return Err(());
                    }
                },
                Err(bytes) => match copy_exact(&bytes, byte_limit) {
                    Ok(copy) => copy,
                    Err(()) => {
                        normalized.push(bytes);
                        normalized.extend(pending);
                        self.slice_store = normalized;
                        return Err(());
                    }
                },
            };
            resident = match resident.checked_add(result.len()) {
                Some(resident) => resident,
                None => {
                    normalized.push(result);
                    normalized.extend(pending);
                    self.slice_store = normalized;
                    return Err(());
                }
            };
            if resident > byte_limit {
                normalized.push(result);
                normalized.extend(pending);
                self.slice_store = normalized;
                return Err(());
            }
            normalized.push(result);
        }
        self.slice_store = normalized;
        Ok(())
    }

    pub fn range(&self, range: ByteRange) -> Option<Cow<'_, [u8]>> {
        if range.end > self.len {
            return None;
        }
        if range.is_empty() {
            return Some(Cow::Borrowed(&[]));
        }
        match &self.storage {
            ChainStorage::Coalesced(bytes) => Some(Cow::Borrowed(&bytes[range.start..range.end])),
            ChainStorage::Slices => {
                let mut offset = 0;
                for bytes in self.slices() {
                    let next = offset + bytes.len();
                    if range.start >= offset && range.end <= next {
                        return Some(Cow::Borrowed(&bytes[range.start - offset..range.end - offset]));
                    }
                    offset = next;
                }
                let mut out = Vec::with_capacity(range.len());
                self.for_each_range(range, |part| out.extend_from_slice(part))?;
                Some(Cow::Owned(out))
            }
        }
    }

    pub fn for_each_range(&self, range: ByteRange, mut visitor: impl FnMut(&[u8])) -> Option<()> {
        if range.end > self.len {
            return None;
        }
        match &self.storage {
            ChainStorage::Coalesced(bytes) => visitor(&bytes[range.start..range.end]),
            ChainStorage::Slices => {
                let mut offset = 0;
                for bytes in self.slices() {
                    let next = offset + bytes.len();
                    let start = range.start.max(offset);
                    let end = range.end.min(next);
                    if start < end {
                        visitor(&bytes[start - offset..end - offset]);
                    }
                    if next >= range.end {
                        break;
                    }
                    offset = next;
                }
            }
        }
        Some(())
    }

    pub fn read_range(&self, range: ByteRange, consumed: usize, output: &mut [u8]) -> Option<usize> {
        if range.end > self.len || consumed > range.len() {
            return None;
        }
        let remaining = range.len() - consumed;
        let wanted = remaining.min(output.len());
        if wanted == 0 {
            return Some(0);
        }
        let read = ByteRange::new(range.start + consumed, range.start + consumed + wanted);
        let mut written = 0;
        self.for_each_range(read, |part| {
            output[written..written + part.len()].copy_from_slice(part);
            written += part.len();
        })?;
        Some(written)
    }

    pub fn raw_slices(&self) -> Vec<Bytes> {
        match &self.storage {
            ChainStorage::Slices => self.slices().to_vec(),
            ChainStorage::Coalesced(bytes) => vec![Bytes::copy_from_slice(bytes)],
        }
    }

    pub(super) fn take_raw_slices(&mut self) -> Vec<Bytes> {
        self.len = 0;
        self.coalesce_count = 0;
        self.copied_bytes = 0;
        match std::mem::take(&mut self.storage) {
            ChainStorage::Slices => std::mem::take(&mut self.slice_store),
            ChainStorage::Coalesced(bytes) => vec![bytes.freeze()],
        }
    }

    pub(super) fn coalesce_count(&self) -> usize {
        self.coalesce_count
    }

    pub(super) fn copied_bytes(&self) -> usize {
        self.copied_bytes
    }

    fn slices(&self) -> &[Bytes] {
        // Slices are kept outside the enum to avoid reallocating the enum on
        // every ordinary append. SAFETY is not used: the field is provided by
        // the compact side table below.
        self.slice_store.as_slice()
    }

    fn slices_mut(&mut self) -> &mut Vec<Bytes> {
        &mut self.slice_store
    }
}

fn copy_exact(bytes: &[u8], byte_limit: usize) -> Result<Bytes, ()> {
    if bytes.len() > byte_limit {
        return Err(());
    }
    let mut normalized = BytesMut::with_capacity(bytes.len());
    if normalized.capacity() > byte_limit {
        return Err(());
    }
    normalized.extend_from_slice(bytes);
    Ok(normalized.freeze())
}
