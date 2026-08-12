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
pub(super) struct SliceChain {
    storage: ChainStorage,
    len: usize,
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
    resident_capacity: usize,
    handle_count: usize,
    metadata_capacity: usize,
    scratch_resident: usize,
    scratch_metadata: usize,
}

enum PlannedStorage {
    Slices(Vec<StoredSlice>),
    AppendSlices(Vec<StoredSlice>),
    Coalesced(BytesMut),
    AppendCoalesced(Bytes),
    Noop,
}

impl ChainAppendPlan {
    pub(super) fn resident_capacity(&self) -> usize {
        self.resident_capacity
    }

    pub(super) fn handle_count(&self) -> usize {
        self.handle_count
    }

    pub(super) fn metadata_capacity(&self) -> usize {
        self.metadata_capacity
    }

    pub(super) fn scratch_resident(&self) -> usize {
        self.scratch_resident
    }

    pub(super) fn scratch_metadata(&self) -> usize {
        self.scratch_metadata
    }
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

    /// General decoder append. Window code uses [`Self::plan_append`] so a
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

    pub(super) fn plan_append(
        &self,
        incoming: Vec<Bytes>,
        logical_limit: usize,
        resident_limit: usize,
    ) -> Result<ChainAppendPlan, ChainLimit> {
        let incoming_len = incoming.iter().try_fold(0usize, |total, bytes| {
            total.checked_add(bytes.len()).ok_or(ChainLimit::Logical)
        })?;
        let new_len = self.len.checked_add(incoming_len).ok_or(ChainLimit::Logical)?;
        if new_len > logical_limit {
            return Err(ChainLimit::Logical);
        }
        let incoming_handles = incoming.iter().filter(|bytes| !bytes.is_empty()).count();

        // Decide the representation before allocating any StoredSlice table.
        // A hostile record with many tiny handles goes directly into one
        // bounded arena; the caller-owned input Vec is consumed in place.
        if let ChainStorage::Slices(current) = &self.storage {
            if current.len().saturating_add(incoming_handles) > MAX_SLICE_HANDLES {
                let mut arena = BytesMut::with_capacity(logical_limit);
                if arena.capacity() > resident_limit {
                    return Err(ChainLimit::Resident);
                }
                for slice in current {
                    arena.extend_from_slice(&slice.bytes);
                }
                for bytes in incoming {
                    arena.extend_from_slice(&bytes);
                }
                let resident_capacity = arena.capacity();
                return Ok(ChainAppendPlan {
                    storage: PlannedStorage::Coalesced(arena),
                    len: new_len,
                    resident_capacity,
                    handle_count: usize::from(new_len != 0),
                    metadata_capacity: 0,
                    scratch_resident: resident_capacity,
                    scratch_metadata: 0,
                });
            }
        }
        if let ChainStorage::Coalesced(current) = &self.storage {
            if new_len > current.capacity() || current.capacity() > resident_limit {
                return Err(ChainLimit::Resident);
            }
            if incoming_len == 0 {
                return Ok(ChainAppendPlan {
                    storage: PlannedStorage::Noop,
                    len: new_len,
                    resident_capacity: current.capacity(),
                    handle_count: usize::from(new_len != 0),
                    metadata_capacity: 0,
                    scratch_resident: 0,
                    scratch_metadata: 0,
                });
            }
            let mut compact = BytesMut::with_capacity(incoming_len);
            if compact.capacity() > resident_limit {
                return Err(ChainLimit::Resident);
            }
            for bytes in incoming {
                compact.extend_from_slice(&bytes);
            }
            let scratch_resident = compact.capacity();
            return Ok(ChainAppendPlan {
                storage: PlannedStorage::AppendCoalesced(compact.freeze()),
                len: new_len,
                resident_capacity: current.capacity(),
                handle_count: usize::from(new_len != 0),
                metadata_capacity: 0,
                scratch_resident,
                scratch_metadata: 0,
            });
        }

        let normalized = normalize_slices(incoming, resident_limit)?;
        let incoming_resident = normalized.iter().try_fold(0usize, |total, slice| {
            total.checked_add(slice.resident_capacity).ok_or(ChainLimit::Resident)
        })?;
        let incoming_metadata = normalized
            .capacity()
            .checked_mul(size_of::<StoredSlice>())
            .ok_or(ChainLimit::Resident)?;

        match &self.storage {
            ChainStorage::Slices(current)
                if current.len() + normalized.len() <= MAX_SLICE_HANDLES && current.capacity() < MAX_SLICE_HANDLES =>
            {
                let normalized_len = normalized.len();
                // One fixed handle table makes every later retained fragment
                // an O(1) append until the one-time coalesce threshold.
                let mut slices = Vec::with_capacity(MAX_SLICE_HANDLES);
                slices.extend(current.iter().cloned());
                slices.extend(normalized);
                let resident_capacity = slices.iter().try_fold(0usize, |total, slice| {
                    total.checked_add(slice.resident_capacity).ok_or(ChainLimit::Resident)
                })?;
                if resident_capacity > resident_limit {
                    return Err(ChainLimit::Resident);
                }
                let metadata_capacity = slices
                    .capacity()
                    .checked_mul(size_of::<StoredSlice>())
                    .ok_or(ChainLimit::Resident)?;
                Ok(ChainAppendPlan {
                    storage: PlannedStorage::Slices(slices),
                    len: new_len,
                    resident_capacity,
                    handle_count: current.len() + normalized_len,
                    metadata_capacity,
                    scratch_resident: resident_capacity,
                    scratch_metadata: 0,
                })
            }
            ChainStorage::Slices(current) if current.len() + normalized.len() <= MAX_SLICE_HANDLES => {
                let handle_count = current.len() + normalized.len();
                let resident_capacity = self
                    .resident_capacity()
                    .checked_add(incoming_resident)
                    .ok_or(ChainLimit::Resident)?;
                if resident_capacity > resident_limit {
                    return Err(ChainLimit::Resident);
                }
                Ok(ChainAppendPlan {
                    storage: PlannedStorage::AppendSlices(normalized),
                    len: new_len,
                    resident_capacity,
                    handle_count,
                    metadata_capacity: current.capacity().saturating_mul(size_of::<StoredSlice>()),
                    scratch_resident: incoming_resident,
                    scratch_metadata: incoming_metadata,
                })
            }
            ChainStorage::Slices(_) | ChainStorage::Coalesced(_) => {
                unreachable!("representation branches were decided before normalization")
            }
        }
    }

    pub(super) fn commit_append(&mut self, plan: ChainAppendPlan) {
        match plan.storage {
            PlannedStorage::Slices(slices) => self.storage = ChainStorage::Slices(slices),
            PlannedStorage::AppendSlices(incoming) => {
                let ChainStorage::Slices(current) = &mut self.storage else {
                    unreachable!("append plan matches the chain representation")
                };
                debug_assert!(current.capacity() - current.len() >= incoming.len());
                current.extend(incoming);
            }
            PlannedStorage::Coalesced(arena) => {
                self.coalesce_count = self.coalesce_count.saturating_add(1);
                self.copied_bytes = self.copied_bytes.saturating_add(plan.len);
                self.storage = ChainStorage::Coalesced(arena);
            }
            PlannedStorage::AppendCoalesced(incoming) => {
                let ChainStorage::Coalesced(current) = &mut self.storage else {
                    unreachable!("append plan matches the chain representation")
                };
                current.extend_from_slice(&incoming);
                self.copied_bytes = self.copied_bytes.saturating_add(incoming.len());
            }
            PlannedStorage::Noop => {}
        }
        self.len = plan.len;
    }

    /// Normalize only retained handles, committing only after every slice and
    /// resident check succeeds.
    pub fn normalize_retained(&mut self, byte_limit: usize) -> Result<(), ()> {
        let ChainStorage::Slices(current) = &self.storage else {
            return Ok(());
        };
        let source: Vec<Bytes> = current.iter().map(|slice| slice.bytes.clone()).collect();
        let normalized = normalize_slices(source, byte_limit).map_err(|_| ())?;
        self.storage = ChainStorage::Slices(normalized);
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

    pub(super) fn take_raw_slices(&mut self) -> Vec<Bytes> {
        let output = Vec::with_capacity(self.handle_count());
        self.take_raw_slices_into(output)
    }

    pub(super) fn take_raw_slices_into(&mut self, mut output: Vec<Bytes>) -> Vec<Bytes> {
        debug_assert!(output.capacity() >= self.handle_count());
        self.len = 0;
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
