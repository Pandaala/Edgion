use std::borrow::Cow;
use std::mem::size_of;

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

/// A retained handle and the visible capacity observed when it was normalized.
#[derive(Debug, Clone)]
struct StoredSlice {
    bytes: Bytes,
    resident_capacity: usize,
}

#[derive(Debug, Default)]
pub(crate) struct SliceChain {
    storage: ChainStorage,
    len: usize,
    normalized_slice_prefix: usize,
    coalesce_count: usize,
    copied_bytes: usize,
}

#[derive(Debug)]
enum ChainStorage {
    Slices(Vec<StoredSlice>),
    Coalesced(BytesMut),
}

impl Default for ChainStorage {
    fn default() -> Self {
        Self::Slices(Vec::new())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ChainLimit {
    Logical,
    Resident,
}

/// Fully prepared append. Construction never mutates the live chain; commit
/// cannot fail and never recopies an existing coalesced prefix.
pub(super) struct ChainAppendPlan {
    storage: PlannedStorage,
    len: usize,
}

/// Allocation-free admission data. Additional storage excludes the live table
/// reused by AppendSlices and the arena reused by AppendCoalesced.
pub(super) struct ChainAppendLayout {
    kind: AppendKind,
    len: usize,
    pub resident: usize,
    pub handles: usize,
    pub metadata: usize,
    pub additional_resident: usize,
    pub additional_metadata: usize,
}

enum AppendKind {
    NewTable,
    AppendSlices,
    Coalesce,
    AppendCoalesced,
}

enum PlannedStorage {
    Slices(Vec<StoredSlice>),
    AppendSlices(Vec<StoredSlice>),
    Coalesced(BytesMut),
    AppendCoalesced(Vec<Bytes>),
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
            ChainStorage::Slices(slices) => slices.len(),
            ChainStorage::Coalesced(bytes) => usize::from(!bytes.is_empty()),
        }
    }

    pub(super) fn resident_capacity(&self) -> usize {
        match &self.storage {
            ChainStorage::Slices(slices) => slices.iter().map(|slice| slice.resident_capacity).sum(),
            ChainStorage::Coalesced(bytes) => bytes.capacity(),
        }
    }

    /// Metadata bytes, already multiplied by the entry size exactly once.
    pub(super) fn slice_metadata_capacity(&self) -> usize {
        match &self.storage {
            ChainStorage::Slices(slices) => slices.capacity().saturating_mul(size_of::<StoredSlice>()),
            ChainStorage::Coalesced(_) => 0,
        }
    }

    /// General decoder append. Window code uses [`Self::inspect_append`] so a
    /// rejected append cannot alter bytes or accounting.
    pub fn push(&mut self, bytes: Bytes, byte_limit: usize) -> Result<(), ()> {
        if bytes.is_empty() {
            return Ok(());
        }
        let new_len = self.len.checked_add(bytes.len()).ok_or(())?;
        if new_len > byte_limit {
            return Err(());
        }
        match &mut self.storage {
            ChainStorage::Slices(slices) if slices.len() < MAX_SLICE_HANDLES => {
                let resident_capacity = bytes.len();
                slices.push(StoredSlice {
                    bytes,
                    resident_capacity,
                });
            }
            ChainStorage::Slices(slices) => {
                let mut arena = BytesMut::with_capacity(byte_limit);
                if arena.capacity() > byte_limit {
                    return Err(());
                }
                for slice in slices.iter() {
                    arena.extend_from_slice(&slice.bytes);
                }
                arena.extend_from_slice(&bytes);
                self.copied_bytes = self.copied_bytes.saturating_add(new_len);
                self.coalesce_count = self.coalesce_count.saturating_add(1);
                self.storage = ChainStorage::Coalesced(arena);
                self.normalized_slice_prefix = 0;
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

    pub(super) fn inspect_append(
        &self,
        incoming: &[Bytes],
        logical_limit: usize,
        resident_limit: usize,
    ) -> Result<ChainAppendLayout, ChainLimit> {
        let incoming_len = incoming.iter().try_fold(0usize, |total, bytes| {
            total.checked_add(bytes.len()).ok_or(ChainLimit::Logical)
        })?;
        let len = self.len.checked_add(incoming_len).ok_or(ChainLimit::Logical)?;
        if len > logical_limit {
            return Err(ChainLimit::Logical);
        }
        let incoming_handles = incoming.iter().filter(|bytes| !bytes.is_empty()).count();
        let layout = match &self.storage {
            ChainStorage::Slices(current) if incoming_handles > MAX_SLICE_HANDLES.saturating_sub(current.len()) => {
                ChainAppendLayout {
                    kind: AppendKind::Coalesce,
                    len,
                    resident: logical_limit,
                    handles: usize::from(len != 0),
                    metadata: 0,
                    additional_resident: logical_limit,
                    additional_metadata: 0,
                }
            }
            ChainStorage::Slices(current) => {
                let new_table = current.capacity() < MAX_SLICE_HANDLES && incoming_handles != 0;
                ChainAppendLayout {
                    kind: if new_table {
                        AppendKind::NewTable
                    } else {
                        AppendKind::AppendSlices
                    },
                    len,
                    resident: self
                        .resident_capacity()
                        .checked_add(incoming_len)
                        .ok_or(ChainLimit::Resident)?,
                    handles: current.len() + incoming_handles,
                    metadata: if new_table {
                        MAX_SLICE_HANDLES * size_of::<StoredSlice>()
                    } else {
                        self.slice_metadata_capacity()
                    },
                    additional_resident: incoming_len,
                    additional_metadata: if new_table { MAX_SLICE_HANDLES } else { incoming_handles }
                        * size_of::<StoredSlice>(),
                }
            }
            ChainStorage::Coalesced(current) => {
                if len > current.capacity() {
                    return Err(ChainLimit::Resident);
                }
                ChainAppendLayout {
                    kind: AppendKind::AppendCoalesced,
                    len,
                    resident: current.capacity(),
                    handles: usize::from(len != 0),
                    metadata: 0,
                    additional_resident: 0,
                    additional_metadata: 0,
                }
            }
        };
        if layout.resident > resident_limit {
            return Err(ChainLimit::Resident);
        }
        Ok(layout)
    }

    /// The caller admits every layout allocation before transferring the input.
    /// No validation or capacity growth remains after this point.
    pub(super) fn prepare_append(&self, incoming: Vec<Bytes>, layout: ChainAppendLayout) -> ChainAppendPlan {
        let storage = match layout.kind {
            AppendKind::Coalesce => {
                let mut arena = BytesMut::with_capacity(layout.resident);
                self.for_each_range(ByteRange::new(0, self.len), |part| arena.extend_from_slice(part));
                for bytes in incoming {
                    arena.extend_from_slice(&bytes);
                }
                PlannedStorage::Coalesced(arena)
            }
            AppendKind::AppendCoalesced => PlannedStorage::AppendCoalesced(incoming),
            AppendKind::NewTable | AppendKind::AppendSlices => {
                let mut slices = Vec::with_capacity(layout.additional_metadata / size_of::<StoredSlice>());
                if matches!(layout.kind, AppendKind::NewTable) {
                    if let ChainStorage::Slices(current) = &self.storage {
                        slices.extend(current.iter().cloned());
                    }
                }
                for original in incoming {
                    if original.is_empty() {
                        continue;
                    }
                    let len = original.len();
                    let bytes = match original.try_into_mut() {
                        Ok(bytes) if bytes.capacity() == len => bytes.freeze(),
                        Ok(bytes) => Bytes::copy_from_slice(&bytes),
                        Err(bytes) => Bytes::copy_from_slice(&bytes),
                    };
                    slices.push(StoredSlice {
                        bytes,
                        resident_capacity: len,
                    });
                }
                if matches!(layout.kind, AppendKind::NewTable) {
                    PlannedStorage::Slices(slices)
                } else {
                    PlannedStorage::AppendSlices(slices)
                }
            }
        };
        ChainAppendPlan {
            storage,
            len: layout.len,
        }
    }

    pub(super) fn commit_append(&mut self, plan: ChainAppendPlan) {
        match plan.storage {
            PlannedStorage::Slices(slices) => {
                self.normalized_slice_prefix = slices.len();
                self.storage = ChainStorage::Slices(slices);
            }
            PlannedStorage::AppendSlices(incoming) => {
                let ChainStorage::Slices(current) = &mut self.storage else {
                    unreachable!("append plan matches the chain representation")
                };
                debug_assert!(current.capacity() - current.len() >= incoming.len());
                current.extend(incoming);
                self.normalized_slice_prefix = current.len();
            }
            PlannedStorage::Coalesced(arena) => {
                self.coalesce_count = self.coalesce_count.saturating_add(1);
                self.copied_bytes = self.copied_bytes.saturating_add(plan.len);
                self.storage = ChainStorage::Coalesced(arena);
                self.normalized_slice_prefix = 0;
            }
            PlannedStorage::AppendCoalesced(incoming) => {
                let ChainStorage::Coalesced(current) = &mut self.storage else {
                    unreachable!("append plan matches the chain representation")
                };
                for bytes in incoming {
                    current.extend_from_slice(&bytes);
                    self.copied_bytes = self.copied_bytes.saturating_add(bytes.len());
                }
                self.normalized_slice_prefix = 0;
            }
        }
        self.len = plan.len;
    }

    pub(super) fn clear(&mut self) {
        *self = Self::default();
    }

    /// Normalize only retained handles, committing only after every slice and
    /// resident check succeeds.
    pub fn normalize_retained(&mut self, byte_limit: usize) -> Result<(), ()> {
        let ChainStorage::Slices(current) = &self.storage else {
            return Ok(());
        };
        if self.normalized_slice_prefix > current.len() {
            return Err(());
        }
        let prefix_resident = current[..self.normalized_slice_prefix]
            .iter()
            .try_fold(0usize, |total, slice| total.checked_add(slice.resident_capacity))
            .ok_or(())?;
        let remaining = byte_limit.checked_sub(prefix_resident).ok_or(())?;
        if self.normalized_slice_prefix == current.len() {
            return Ok(());
        }
        let source = current[self.normalized_slice_prefix..]
            .iter()
            .map(|slice| slice.bytes.clone())
            .collect();
        let normalized = normalize_slices(source, remaining).map_err(|_| ())?;

        let ChainStorage::Slices(current) = &mut self.storage else {
            unreachable!("normalization does not change the live representation")
        };
        current.truncate(self.normalized_slice_prefix);
        current.extend(normalized);
        self.normalized_slice_prefix = current.len();
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
            ChainStorage::Slices(slices) => {
                let mut offset = 0;
                for slice in slices {
                    let next = offset + slice.bytes.len();
                    if range.start >= offset && range.end <= next {
                        return Some(Cow::Borrowed(&slice.bytes[range.start - offset..range.end - offset]));
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
            ChainStorage::Slices(slices) => {
                let mut offset = 0;
                for slice in slices {
                    let next = offset + slice.bytes.len();
                    let start = range.start.max(offset);
                    let end = range.end.min(next);
                    if start < end {
                        visitor(&slice.bytes[start - offset..end - offset]);
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
        let wanted = (range.len() - consumed).min(output.len());
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
            ChainStorage::Slices(slices) => slices.iter().map(|slice| slice.bytes.clone()).collect(),
            ChainStorage::Coalesced(bytes) => vec![Bytes::copy_from_slice(bytes)],
        }
    }

    pub(super) fn take_raw_slices_into(&mut self, mut output: Vec<Bytes>) -> Vec<Bytes> {
        debug_assert!(output.capacity() >= self.handle_count());
        self.len = 0;
        self.normalized_slice_prefix = 0;
        self.coalesce_count = 0;
        self.copied_bytes = 0;
        match std::mem::take(&mut self.storage) {
            ChainStorage::Slices(slices) => {
                output.extend(slices.into_iter().map(|slice| slice.bytes));
            }
            ChainStorage::Coalesced(bytes) => output.push(bytes.freeze()),
        }
        output
    }

    #[cfg(test)]
    pub(super) fn coalesce_count(&self) -> usize {
        self.coalesce_count
    }

    #[cfg(test)]
    pub(super) fn copied_bytes(&self) -> usize {
        self.copied_bytes
    }
}

fn normalize_slices(source: Vec<Bytes>, byte_limit: usize) -> Result<Vec<StoredSlice>, ChainLimit> {
    let handle_count = source.iter().filter(|bytes| !bytes.is_empty()).count();
    debug_assert!(handle_count <= MAX_SLICE_HANDLES);
    let mut normalized = Vec::with_capacity(handle_count);
    let mut resident = 0usize;
    for original in source {
        if original.is_empty() {
            continue;
        }
        let slice = normalize_slice(original, byte_limit)?;
        resident = resident
            .checked_add(slice.resident_capacity)
            .ok_or(ChainLimit::Resident)?;
        if resident > byte_limit {
            return Err(ChainLimit::Resident);
        }
        normalized.push(slice);
    }
    Ok(normalized)
}

fn normalize_slice(original: Bytes, byte_limit: usize) -> Result<StoredSlice, ChainLimit> {
    if original.len() > byte_limit {
        return Err(ChainLimit::Resident);
    }
    match original.try_into_mut() {
        Ok(bytes) if bytes.capacity() == bytes.len() => {
            let resident_capacity = bytes.capacity();
            Ok(StoredSlice {
                bytes: bytes.freeze(),
                resident_capacity,
            })
        }
        Ok(bytes) => copy_exact_stored(&bytes, byte_limit),
        Err(bytes) => copy_exact_stored(&bytes, byte_limit),
    }
}

fn copy_exact_stored(bytes: &[u8], byte_limit: usize) -> Result<StoredSlice, ChainLimit> {
    let mut normalized = BytesMut::with_capacity(bytes.len());
    if normalized.capacity() > byte_limit {
        return Err(ChainLimit::Resident);
    }
    normalized.extend_from_slice(bytes);
    let resident_capacity = normalized.capacity();
    Ok(StoredSlice {
        bytes: normalized.freeze(),
        resident_capacity,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slack_slice(byte: u8) -> Bytes {
        let mut bytes = BytesMut::with_capacity(64);
        bytes.extend_from_slice(&[byte]);
        bytes.freeze()
    }

    #[test]
    fn repeated_normalization_near_handle_cap_preserves_prior_allocations() {
        let mut chain = SliceChain::default();
        let mut retained_pointers = Vec::new();

        for byte in 0..MAX_SLICE_HANDLES {
            chain.push(slack_slice(byte as u8), 1024).unwrap();
            chain.normalize_retained(1024).unwrap();

            let slices = chain.raw_slices();
            assert_eq!(slices.len(), byte + 1);
            for (slice, pointer) in slices.iter().zip(&retained_pointers) {
                assert_eq!(slice.as_ptr(), *pointer, "a previously normalized slice was recopied");
            }
            retained_pointers.push(slices.last().unwrap().as_ptr());
        }

        assert_eq!(chain.coalesce_count(), 0);
        assert_eq!(chain.copied_bytes(), 0);
        assert_eq!(chain.len(), MAX_SLICE_HANDLES);

        chain.push(Bytes::from_static(b"x"), 1024).unwrap();
        assert_eq!(chain.coalesce_count(), 1);
        chain.normalize_retained(1024).unwrap();
        assert_eq!(chain.handle_count(), 1);
        assert_eq!(chain.len(), MAX_SLICE_HANDLES + 1);
    }

    #[test]
    fn failed_suffix_normalization_is_atomic_for_prefix_and_new_handle() {
        let mut chain = SliceChain::default();
        chain.push(slack_slice(b'a'), 8).unwrap();
        chain.normalize_retained(8).unwrap();
        chain.push(slack_slice(b'b'), 8).unwrap();

        let before = chain.raw_slices();
        assert!(chain.normalize_retained(1).is_err());
        let after = chain.raw_slices();

        assert_eq!(after, before);
        assert_eq!(after[0].as_ptr(), before[0].as_ptr());
        assert_eq!(after[1].as_ptr(), before[1].as_ptr());
        assert_eq!(chain.len(), 2);
    }

    #[test]
    fn empty_and_already_normalized_chains_are_noops() {
        let mut empty = SliceChain::default();
        empty.normalize_retained(0).unwrap();

        let mut chain = SliceChain::default();
        chain.push(slack_slice(b'a'), 8).unwrap();
        chain.normalize_retained(8).unwrap();
        let pointer = chain.raw_slices()[0].as_ptr();
        let resident = chain.resident_capacity();
        chain.normalize_retained(resident).unwrap();
        assert_eq!(chain.raw_slices()[0].as_ptr(), pointer);
        assert!(chain.normalize_retained(resident - 1).is_err());
        assert_eq!(chain.raw_slices()[0].as_ptr(), pointer);
    }
}
