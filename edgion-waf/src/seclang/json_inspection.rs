//! Bounded sparse JSON storage shared by inspection and canonical replay.
#[cfg(test)]
use crate::seclang::measurements::{self, Event};

use std::mem::size_of;
use std::num::{NonZeroU32, NonZeroUsize};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_STORE: AtomicU64 = AtomicU64::new(1);
fn allocate_owner(counter: &AtomicU64) -> Result<u64> {
    counter
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
        .map_err(|_| StoreError::Generation)
}

use super::super::json_inspection_plan::JsonView;
use super::super::InspectionDomain;
use super::*;
use crate::seclang::{JsonScalarKind, JsonSemanticSummary, XmlSemanticSummary};

#[derive(Debug, Clone, Copy)]
pub struct ResultLimits {
    pub candidates: usize,
    pub records: usize,
    pub retained_bytes: usize,
    pub work: usize,
    pub deferred_entries: usize,
    pub deferred_bytes: usize,
}
impl Default for ResultLimits {
    fn default() -> Self {
        Self {
            candidates: 65_536,
            records: 1_000_000,
            retained_bytes: 32 << 20,
            work: 128 << 20,
            deferred_entries: 4096,
            deferred_bytes: 4 << 20,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StoreError {
    Capacity,
    Records,
    Candidates,
    Work,
    Deferred,
    Allocation,
    Arithmetic,
    Identity,
    State,
    Generation,
}
type Result<T> = std::result::Result<T, StoreError>;

#[derive(Debug, Default, Clone, Copy)]
pub(super) struct StoreUsage {
    pub retained_bytes: usize,
    pub peak_retained_bytes: usize,
    pub allocated_records: usize,
    pub live_candidates: usize,
    pub live_profiles: usize,
    pub live_changes: usize,
    pub live_outcomes: usize,
    pub live_captures: usize,
    pub name_bytes: usize,
    pub deferred_bytes: usize,
    pub matched_bytes: usize,
    pub capture_bytes: usize,
    pub profile_metadata_bytes: usize,
    pub deferred_entries: usize,
    pub work: usize,
    pub abandoned: usize,
}

fn add(a: usize, b: usize) -> Result<usize> {
    a.checked_add(b).ok_or(StoreError::Arithmetic)
}
fn mul(a: usize, b: usize) -> Result<usize> {
    a.checked_mul(b).ok_or(StoreError::Arithmetic)
}
fn sub(a: usize, b: usize) -> Result<usize> {
    a.checked_sub(b).ok_or(StoreError::Identity)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RawId {
    owner: u64,
    slot: u32,
    generation: u32,
}
macro_rules! ids {
    ($($name:ident),*) => { $(#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub(super) struct $name(RawId);)* };
}
ids!(CandidateId, ProfileId, OutcomeId, ChangeId, CaptureId);
// Candidate-local links omit the pool owner but preserve the complete slot and
// generation domain. External handles always retain their owner checks.
macro_rules! local_ids {
    ($($local:ident => $full:ident),*) => { $(
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        struct $local {
            slot: u32,
            generation: u32,
        }
        impl $local {
            fn from_full(id: $full, owner: u64) -> Result<Self> {
                if id.0.owner != owner {
                    return Err(StoreError::Identity);
                }
                Ok(Self { slot: id.0.slot, generation: id.0.generation })
            }
            fn full(self, owner: u64) -> $full {
                $full(RawId { owner, slot: self.slot, generation: self.generation })
            }
        }
    )* };
}
local_ids!(LocalCandidateId => CandidateId, LocalProfileId => ProfileId, LocalOutcomeId => OutcomeId);
/// Only the first outcome of one candidate/profile/stage can mint this token.
#[derive(Clone, Copy)]
pub(super) struct StageMatchToken(OutcomeId);
#[derive(Debug, Clone, Copy)]
pub(super) struct Receipt {
    pub head: CandidateId,
    pub tail: CandidateId,
}

struct Slot<T> {
    generation: u32,
    free_next: Option<u32>,
    value: Option<T>,
}
struct Pool<T> {
    owner: u64,
    slots: Vec<Slot<T>>,
    free: Option<u32>,
}
impl<T> Pool<T> {
    fn new(owner: u64) -> Self {
        Self {
            owner,
            slots: Vec::new(),
            free: None,
        }
    }
}

pub(super) struct InspectionBudget {
    owner: u64,
    limits: ResultLimits,
    usage: StoreUsage,
    scratch: usize,
    failure: Option<StoreError>,
}
impl InspectionBudget {
    /// Empty input has no parser/store state but still owns publication buffers.
    pub(super) fn empty_publication(limits: ResultLimits) -> Result<Self> {
        Ok(Self {
            owner: allocate_owner(&NEXT_STORE)?,
            limits,
            usage: StoreUsage::default(),
            scratch: 0,
            failure: None,
        })
    }
    /// Persistent publication storage, never part of releasable inspection scratch.
    /// Only a fresh publication owner may use this exact-size allocation seam.
    pub(super) fn reserve_publication<T>(&mut self, values: &mut Vec<T>, count: usize) -> Result<()> {
        self.checked(self.owner, |budget| {
            if !values.is_empty() || values.capacity() != 0 {
                return Err(StoreError::State);
            }
            let requested = mul(count, size_of::<T>())?;
            budget.peak(add(budget.usage.retained_bytes, requested)?)?;
            budget.work(requested)?;
            values.try_reserve_exact(count).map_err(|_| StoreError::Allocation)?;
            let actual = mul(values.capacity(), size_of::<T>())?;
            let retained = add(budget.usage.retained_bytes, actual)?;
            budget.peak(retained)?;
            budget.usage.retained_bytes = retained;
            Ok(())
        })
    }
    fn checked<T>(&mut self, owner: u64, operation: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        self.work(0)?;
        let result = if self.owner != owner {
            Err(StoreError::Identity)
        } else {
            operation(self)
        };
        if let Err(error) = result {
            self.failure = Some(error);
        }
        result
    }

    pub(super) fn work(&mut self, amount: usize) -> Result<()> {
        if let Some(error) = self.failure {
            return Err(error);
        }
        let result = add(self.usage.work, amount).and_then(|work| {
            self.usage.work = work;
            if work > self.limits.work {
                Err(StoreError::Work)
            } else {
                Ok(())
            }
        });
        if let Err(error) = result {
            self.failure = Some(error);
        }
        result
    }
    fn peak(&mut self, bytes: usize) -> Result<()> {
        if let Some(error) = self.failure {
            return Err(error);
        }
        self.usage.peak_retained_bytes = self.usage.peak_retained_bytes.max(bytes);
        if bytes > self.limits.retained_bytes {
            self.failure = Some(StoreError::Capacity);
            Err(StoreError::Capacity)
        } else {
            Ok(())
        }
    }
    pub(super) fn failure(&self) -> Option<StoreError> {
        self.failure
    }
    /// Read-only admission: fallback must preserve cursor error timing/counters.
    fn admits_work(&self, owner: u64, amount: usize) -> bool {
        self.owner == owner
            && self.failure.is_none()
            && self
                .usage
                .work
                .checked_add(amount)
                .is_some_and(|work| work <= self.limits.work)
    }
    /// Caller invokes this only after dropping all externally owned scratch.
    /// Cleanup does not clear or bypass the first physical failure.
    pub(super) fn release_scratch(&mut self) {
        self.usage.retained_bytes -= self.scratch;
        self.scratch = 0;
    }
    fn grow<T>(&mut self, values: &mut Vec<T>, additional: usize, records: bool, candidate: bool) -> Result<()> {
        let required = add(values.len(), additional)?;
        if required <= values.capacity() {
            return Ok(());
        }
        let old = values.capacity();
        let unit = size_of::<T>();
        let mut maximum = add(
            old,
            self.limits.retained_bytes.saturating_sub(self.usage.retained_bytes) / unit,
        )?;
        if records {
            maximum = maximum.min(add(
                old,
                self.limits.records.saturating_sub(self.usage.allocated_records),
            )?);
        }
        if candidate {
            maximum = maximum.min(self.limits.candidates);
        }
        if required > maximum {
            return Err(if candidate && required > self.limits.candidates {
                StoreError::Candidates
            } else if records && required > add(old, self.limits.records.saturating_sub(self.usage.allocated_records))?
            {
                StoreError::Records
            } else {
                StoreError::Capacity
            });
        }
        // Admit the old allocation plus the complete replacement, including
        // transient realloc overlap. Clip geometric growth when only a smaller
        // replacement fits the same hard physical capacity bound.
        let peak_max = self.limits.retained_bytes.saturating_sub(self.usage.retained_bytes) / unit;
        maximum = maximum.min(peak_max);
        if required > maximum {
            return Err(StoreError::Capacity);
        }
        let target = required.max(old.saturating_mul(2).max(4)).min(maximum);
        self.peak(add(self.usage.retained_bytes, mul(target, unit)?)?)?;
        self.work(mul(values.len(), unit)?)?;
        values
            .try_reserve_exact(target - values.len())
            .map_err(|_| StoreError::Allocation)?;
        let increase = sub(values.capacity(), old)?;
        self.peak(add(self.usage.retained_bytes, mul(values.capacity(), unit)?)?)?;
        self.usage.retained_bytes = add(self.usage.retained_bytes, mul(increase, unit)?)?;
        if records {
            self.usage.allocated_records = add(self.usage.allocated_records, increase)?;
            if self.usage.allocated_records > self.limits.records {
                return Err(StoreError::Records);
            }
        }
        if candidate && values.capacity() > self.limits.candidates {
            return Err(StoreError::Candidates);
        }
        Ok(())
    }
}

impl<T> Pool<T> {
    fn get(&self, id: RawId) -> Result<&T> {
        if id.owner != self.owner {
            return Err(StoreError::Identity);
        }
        let slot = self.slots.get(id.slot as usize).ok_or(StoreError::Identity)?;
        if slot.generation != id.generation {
            return Err(StoreError::Identity);
        }
        slot.value.as_ref().ok_or(StoreError::Identity)
    }
    fn get_mut(&mut self, id: RawId) -> Result<&mut T> {
        if id.owner != self.owner {
            return Err(StoreError::Identity);
        }
        let slot = self.slots.get_mut(id.slot as usize).ok_or(StoreError::Identity)?;
        if slot.generation != id.generation {
            return Err(StoreError::Identity);
        }
        slot.value.as_mut().ok_or(StoreError::Identity)
    }
    fn insert(&mut self, value: T, budget: &mut InspectionBudget, candidate: bool) -> Result<RawId> {
        budget.work(1)?;
        if let Some(index) = self.free {
            let slot = self.slots.get_mut(index as usize).ok_or(StoreError::Identity)?;
            if slot.value.is_some() {
                return Err(StoreError::Identity);
            }
            self.free = slot.free_next;
            slot.free_next = None;
            slot.value = Some(value);
            Ok(RawId {
                owner: self.owner,
                slot: index,
                generation: slot.generation,
            })
        } else {
            let index = u32::try_from(self.slots.len()).map_err(|_| StoreError::Arithmetic)?;
            budget.grow(&mut self.slots, 1, true, candidate)?;
            self.slots.push(Slot {
                generation: 0,
                free_next: None,
                value: Some(value),
            });
            Ok(RawId {
                owner: self.owner,
                slot: index,
                generation: 0,
            })
        }
    }
    fn remove(&mut self, id: RawId, budget: &mut InspectionBudget) -> Result<T> {
        budget.work(1)?;
        self.get(id)?;
        let slot = &mut self.slots[id.slot as usize];
        let generation = slot.generation.checked_add(1).ok_or(StoreError::Generation)?;
        let value = slot.value.take().ok_or(StoreError::Identity)?;
        slot.generation = generation;
        slot.free_next = self.free;
        self.free = Some(id.slot);
        Ok(value)
    }
}

#[derive(Clone, Copy)]
struct Lease {
    owner: u64,
    offset: usize,
    len: usize,
    size: usize,
}
/// Minted only after sealing: the arena never frees or reuses this range again.
/// Keeping a handle does not retain the original body or allocate another owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SealedMatchValue {
    owner: u64,
    offset: usize,
    len: usize,
}
impl SealedMatchValue {
    pub(super) fn len(self) -> usize {
        self.len
    }
}
struct LargeClass {
    size: usize,
    head: Option<usize>,
}
struct BytePool {
    owner: u64,
    bytes: Vec<u8>,
    heads: [Option<usize>; 13],
    large_heads: Vec<LargeClass>,
}
impl BytePool {
    fn new(owner: u64) -> Self {
        Self {
            owner,
            bytes: Vec::new(),
            heads: [None; 13],
            large_heads: Vec::new(),
        }
    }
}
impl BytePool {
    fn allocation_size(len: usize) -> Result<usize> {
        if len == 0 {
            return Ok(0);
        }
        let len = len.max(size_of::<usize>());
        if len <= 4096 {
            len.checked_next_power_of_two().ok_or(StoreError::Arithmetic)
        } else {
            Ok(add(len, 4095)? & !4095)
        }
    }

    // Preserve a small page remainder for seal-time metadata after a large lease.
    // Optional hints must never turn otherwise-admitted exact growth into failure.
    fn arena_reserve_amount(len: usize, capacity: usize, additional: usize, allowance: usize) -> Result<usize> {
        let required = add(len, additional)?;
        if required > 4096 && required > capacity {
            if let Some(rounded) = required.checked_add(4095).map(|bytes| bytes & !4095) {
                if rounded <= allowance {
                    return sub(rounded, len);
                }
            }
        }
        Ok(additional)
    }

    // Large-bin insertion can move metadata, so leases retain size, never this index.
    fn class_index(&mut self, size: usize, register: bool, budget: &mut InspectionBudget) -> Result<usize> {
        if size <= 4096 {
            budget.work(1)?;
            return Ok(size.trailing_zeros() as usize);
        }
        let mut low = 0;
        let mut high = self.large_heads.len();
        while low < high {
            let middle = low + (high - low) / 2;
            budget.work(1)?;
            match self.large_heads[middle].size.cmp(&size) {
                std::cmp::Ordering::Less => low = middle + 1,
                std::cmp::Ordering::Greater => high = middle,
                std::cmp::Ordering::Equal => return add(13, middle),
            }
        }
        if !register {
            return Err(StoreError::Identity);
        }
        // Admit both shifted metadata and the new entry before reserving/inserting.
        budget.work(mul(add(self.large_heads.len() - low, 1)?, size_of::<LargeClass>())?)?;
        budget.grow(&mut self.large_heads, 1, false, false)?;
        self.large_heads.insert(low, LargeClass { size, head: None });
        add(13, low)
    }

    fn head_mut(&mut self, index: usize) -> Result<&mut Option<usize>> {
        if index < 13 {
            self.heads.get_mut(index).ok_or(StoreError::Identity)
        } else {
            self.large_heads
                .get_mut(index - 13)
                .map(|class| &mut class.head)
                .ok_or(StoreError::Identity)
        }
    }

    fn alloc(&mut self, len: usize, budget: &mut InspectionBudget) -> Result<Lease> {
        budget.checked(self.owner, |budget| {
            let size = Self::allocation_size(len)?;
            if size == 0 {
                return Ok(Lease {
                    owner: self.owner,
                    offset: 0,
                    len: 0,
                    size: 0,
                });
            }
            let index = self.class_index(size, true, budget)?;
            let offset = if let Some(offset) = *self.head_mut(index)? {
                self.bytes.get(offset..add(offset, size)?).ok_or(StoreError::Identity)?;
                budget.work(size_of::<usize>())?;
                let mut bytes = [0u8; size_of::<usize>()];
                bytes.copy_from_slice(
                    self.bytes
                        .get(offset..add(offset, size_of::<usize>())?)
                        .ok_or(StoreError::Identity)?,
                );
                let next = usize::from_ne_bytes(bytes);
                *self.head_mut(index)? = if next == usize::MAX { None } else { Some(next) };
                offset
            } else {
                let additional = Self::arena_reserve_amount(
                    self.bytes.len(),
                    self.bytes.capacity(),
                    size,
                    budget.limits.retained_bytes.saturating_sub(budget.usage.retained_bytes),
                )?;
                budget.grow(&mut self.bytes, additional, false, false)?;
                budget.work(size)?;
                let offset = self.bytes.len();
                self.bytes.resize(add(offset, size)?, 0);
                offset
            };
            Ok(Lease {
                owner: self.owner,
                offset,
                len,
                size,
            })
        })
    }
    fn copy(&mut self, input: &[u8], budget: &mut InspectionBudget) -> Result<Lease> {
        let lease = self.alloc(input.len(), budget)?;
        budget.work(input.len())?;
        self.bytes[lease.offset..lease.offset + lease.len].copy_from_slice(input);
        Ok(lease)
    }
    fn zero(&mut self, len: usize, budget: &mut InspectionBudget) -> Result<Lease> {
        let lease = self.alloc(len, budget)?;
        budget.work(len)?;
        self.bytes[lease.offset..lease.offset + lease.len].fill(0);
        Ok(lease)
    }
    fn read(&self, lease: Lease) -> Result<&[u8]> {
        if lease.owner != self.owner || lease.size != Self::allocation_size(lease.len)? {
            return Err(StoreError::Identity);
        }
        self.bytes
            .get(lease.offset..add(lease.offset, lease.len)?)
            .ok_or(StoreError::Identity)
    }
    fn free(&mut self, lease: Lease, budget: &mut InspectionBudget) -> Result<()> {
        budget.checked(self.owner, |budget| {
            if lease.owner != self.owner || lease.size != Self::allocation_size(lease.len)? {
                return Err(StoreError::Identity);
            }
            if lease.len == 0 {
                return Ok(());
            }
            self.bytes
                .get(lease.offset..add(lease.offset, lease.size)?)
                .ok_or(StoreError::Identity)?;
            let index = self.class_index(lease.size, false, budget)?;
            budget.work(size_of::<usize>())?;
            let next = self.head_mut(index)?.unwrap_or(usize::MAX).to_ne_bytes();
            self.bytes
                .get_mut(lease.offset..add(lease.offset, next.len())?)
                .ok_or(StoreError::Identity)?
                .copy_from_slice(&next);
            *self.head_mut(index)? = Some(lease.offset);
            Ok(())
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ProfileState {
    Unseen,
    ImplicitUnchanged,
    RecordedComplete,
    SemanticError,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CandidateKind {
    JsonArgument,
    XmlText,
    XmlAttribute,
}
#[derive(Clone, Copy)]
pub(super) enum InspectionSummary {
    Json(JsonSemanticSummary),
    Xml(XmlSemanticSummary),
}
impl InspectionSummary {
    fn candidate_count(self) -> usize {
        match self {
            Self::Json(s) => s.argument_count,
            Self::Xml(s) => s.value_count,
        }
    }
}
struct Candidate {
    kind: CandidateKind,
    name: Lease,
    semantic_len: usize,
    states: Lease,
    deferred: Option<Lease>,
    profiles: Option<LocalProfileId>,
    outcomes: Option<LocalOutcomeId>,
    next: Option<LocalCandidateId>,
    qualified_name_pair: Option<NonZeroU32>,
    complete: bool,
}
struct Profile {
    owner: CandidateId,
    compiled: usize,
    changes: Option<ChangeId>,
    tail: Option<ChangeId>,
    next: Option<ProfileId>,
    error: Option<CheckpointError>,
}
struct Change {
    owner: ProfileId,
    step: usize,
    output_len: usize,
    next: Option<ChangeId>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CheckpointError {
    pub step: usize,
    pub checkpoint: usize,
    pub error: RuntimeError,
}
struct Outcome {
    owner: CandidateId,
    group: usize,
    stage: usize,
    next: Option<OutcomeId>,
    result: StoredOutcome,
}
enum StoredOutcome {
    Match {
        value: Lease,
        // Outcomes are destroyed only as an entire profile or candidate. Aliases
        // share that lifetime and must neither read nor free bytes during cleanup.
        owns_value: bool,
        captures: Option<CaptureId>,
        tail: Option<CaptureId>,
    },
    Error(CheckpointError),
}
struct Capture {
    owner: OutcomeId,
    key: Lease,
    value: StoredCaptureValue,
    next: Option<CaptureId>,
}

#[derive(Clone, Copy)]
enum StoredCaptureValue {
    Owned(Lease),
    MatchRange { start: usize, len: usize },
}
impl StoredCaptureValue {
    fn len(self) -> usize {
        match self {
            Self::Owned(value) => value.len,
            Self::MatchRange { len, .. } => len,
        }
    }
}

#[derive(Clone, Copy)]
enum Terminal {
    Active,
    Sealed,
    Failed(StoreError),
    Abandoned,
}

/// This token is internal to the collector/store boundary. Its constructor
/// validates the completed applicable profiles; collector dispatch additionally
/// proves that every applicable group has run before requesting it.
pub(super) struct CompletionProof {
    candidate: CandidateId,
}

/// Sparse seal-time names. Candidate identity is checked before following its
/// pair index; immutable spans do not repeat owner or allocation-class metadata.
#[derive(Clone, Copy)]
struct QualifiedSpan {
    offset: NonZeroUsize,
    len: usize,
}
#[derive(Default)]
struct QualifiedNamePair {
    values: [Option<QualifiedSpan>; 2],
}

struct StoreRecords<'generation> {
    domain: InspectionDomain,
    owner: u64,
    generation: GenerationHandle<'generation>,
    active_groups: Vec<bool>,
    named_first: Vec<Option<CandidateId>>,
    qualified_names: Vec<QualifiedNamePair>,
    runtime: RuntimeLimits,
    mode: EvaluationMode,
    candidates: Pool<Candidate>,
    profiles: Pool<Profile>,
    changes: Pool<Change>,
    outcomes: Pool<Outcome>,
    captures: Pool<Capture>,
    bytes: BytePool,
}

impl StoreRecords<'_> {
    fn group_active(&self, group: usize) -> Result<bool> {
        self.plan().groups.get(group).ok_or(StoreError::Identity)?;
        if self.active_groups.is_empty() {
            Ok(true)
        } else {
            self.active_groups.get(group).copied().ok_or(StoreError::Identity)
        }
    }
    fn plan(&self) -> &super::super::json_inspection_plan::CompiledJsonPlan {
        self.generation.inspection_plan(self.domain)
    }
}

pub(super) struct ResultStore<'generation> {
    records: StoreRecords<'generation>,
    budget: InspectionBudget,
    terminal: Terminal,
    sealed: Option<Receipt>,
    summary: Option<InspectionSummary>,
    summary_no_hit: bool,
}

pub(super) struct SealedResults<'generation> {
    records: StoreRecords<'generation>,
    pub(super) receipt: Option<Receipt>,
    pub(super) summary: InspectionSummary,
    summary_no_hit: bool,
}

impl<'generation> ResultStore<'generation> {
    pub(super) fn domain(&self) -> InspectionDomain {
        self.records.domain
    }
    pub(super) fn usage_snapshot(&self) -> StoreUsage {
        self.budget.usage
    }
    pub(super) fn new_domain(
        generation: GenerationHandle<'generation>,
        runtime: RuntimeLimits,
        mode: EvaluationMode,
        limits: ResultLimits,
        domain: InspectionDomain,
    ) -> Result<Self> {
        let owner = allocate_owner(&NEXT_STORE)?;
        Ok(Self {
            records: StoreRecords {
                domain,
                owner,
                generation,
                active_groups: Vec::new(),
                named_first: Vec::new(),
                qualified_names: Vec::new(),
                runtime,
                mode,
                candidates: Pool::new(owner),
                profiles: Pool::new(owner),
                changes: Pool::new(owner),
                outcomes: Pool::new(owner),
                captures: Pool::new(owner),
                bytes: BytePool::new(owner),
            },
            budget: InspectionBudget {
                owner,
                limits,
                usage: StoreUsage::default(),
                scratch: 0,
                failure: None,
            },
            terminal: Terminal::Active,
            sealed: None,
            summary: None,
            summary_no_hit: false,
        })
    }
    fn active<T>(&mut self, operation: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        match self.terminal {
            Terminal::Active => {}
            Terminal::Failed(error) => return Err(error),
            _ => return Err(StoreError::State),
        }
        let result = operation(self);
        if let Err(error) = result {
            self.abandon();
            self.terminal = Terminal::Failed(error);
        }
        result
    }
    /// Empty storage means all groups active. Otherwise retain one exact mask
    /// owner through sealing; absent facts must never become ordinary misses.
    pub(super) fn initialize_activation(&mut self, count: usize) -> Result<()> {
        self.active(|store| {
            if count != store.records.plan().groups.len() {
                return Err(StoreError::Identity);
            }
            store
                .budget
                .reserve_publication(&mut store.records.active_groups, count)?;
            store.budget.work(count)?;
            store.records.active_groups.resize(count, false);
            Ok(())
        })
    }
    pub(super) fn activate_group(&mut self, group: usize) -> Result<()> {
        self.active(|store| {
            store.budget.work(1)?;
            *store.records.active_groups.get_mut(group).ok_or(StoreError::Identity)? = true;
            Ok(())
        })
    }
    /// Collector filtering is allowed to observe false; canonical lookup is not.
    pub(super) fn is_group_active(&mut self, group: usize) -> Result<bool> {
        self.active(|store| {
            store.budget.work(1)?;
            store.records.group_active(group)
        })
    }
    pub(super) fn begin_candidate(
        &mut self,
        name: &[u8],
        semantic_len: usize,
        _kind: JsonScalarKind,
        deferred: Option<&[u8]>,
    ) -> Result<CandidateId> {
        self.begin_value(name, semantic_len, deferred, CandidateKind::JsonArgument)
    }
    pub(super) fn begin_xml_candidate(&mut self, name: Option<&[u8]>, semantic_len: usize) -> Result<CandidateId> {
        self.begin_value(
            name.unwrap_or_default(),
            semantic_len,
            None,
            if name.is_some() {
                CandidateKind::XmlAttribute
            } else {
                CandidateKind::XmlText
            },
        )
    }
    fn begin_value(
        &mut self,
        name: &[u8],
        semantic_len: usize,
        deferred: Option<&[u8]>,
        kind: CandidateKind,
    ) -> Result<CandidateId> {
        if (kind == CandidateKind::JsonArgument) != (self.records.domain == InspectionDomain::Json) {
            return Err(StoreError::Identity);
        }
        self.active(|store| {
            if let Some(value) = deferred {
                if value.len() != semantic_len {
                    return Err(StoreError::Identity);
                }
                if add(store.budget.usage.deferred_entries, 1)? > store.budget.limits.deferred_entries
                    || add(store.budget.usage.deferred_bytes, value.len())? > store.budget.limits.deferred_bytes
                {
                    return Err(StoreError::Deferred);
                }
            }
            let name_lease = store.records.bytes.copy(name, &mut store.budget)?;
            #[cfg(test)]
            measurements::record(Event::NameCopyBytes, name.len());
            let state_len = add(store.records.plan().profiles.len(), 3)? / 4;
            let states = store.records.bytes.zero(state_len, &mut store.budget)?;
            let deferred_lease = deferred
                .map(|value| {
                    let lease = store.records.bytes.copy(value, &mut store.budget)?;
                    #[cfg(test)]
                    measurements::record(Event::DeferredCopyBytes, value.len());
                    Ok(lease)
                })
                .transpose()?;
            let id = CandidateId(store.records.candidates.insert(
                Candidate {
                    kind,
                    name: name_lease,
                    semantic_len,
                    states,
                    deferred: deferred_lease,
                    profiles: None,
                    outcomes: None,
                    next: None,
                    qualified_name_pair: None,
                    complete: false,
                },
                &mut store.budget,
                true,
            )?);
            store.budget.usage.live_candidates += 1;
            #[cfg(test)]
            measurements::record(Event::Candidates, 1);
            store.budget.usage.name_bytes = add(store.budget.usage.name_bytes, name.len())?;
            store.budget.usage.profile_metadata_bytes = add(store.budget.usage.profile_metadata_bytes, state_len)?;
            if let Some(value) = deferred {
                store.budget.usage.deferred_entries += 1;
                store.budget.usage.deferred_bytes = add(store.budget.usage.deferred_bytes, value.len())?;
            }
            Ok(id)
        })
    }
    fn profile_state(&self, id: CandidateId, profile: usize) -> Result<ProfileState> {
        if profile >= self.records.plan().profiles.len() {
            return Err(StoreError::Identity);
        }
        let states = self.records.bytes.read(self.records.candidates.get(id.0)?.states)?;
        match (states[profile / 4] >> ((profile % 4) * 2)) & 3 {
            0 => Ok(ProfileState::Unseen),
            1 => Ok(ProfileState::ImplicitUnchanged),
            2 => Ok(ProfileState::RecordedComplete),
            _ => Ok(ProfileState::SemanticError),
        }
    }
    fn set_profile_state(&mut self, id: CandidateId, profile: usize, state: ProfileState) -> Result<()> {
        if self.profile_state(id, profile)? != ProfileState::Unseen {
            return Err(StoreError::State);
        }
        self.budget.work(1)?;
        let lease = self.records.candidates.get(id.0)?.states;
        let code = match state {
            ProfileState::Unseen => return Err(StoreError::State),
            ProfileState::ImplicitUnchanged => 1,
            ProfileState::RecordedComplete => 2,
            ProfileState::SemanticError => 3,
        };
        self.records.bytes.bytes[lease.offset + profile / 4] |= code << ((profile % 4) * 2);
        Ok(())
    }
    fn find_profile(&mut self, owner: CandidateId, compiled: usize) -> Result<Option<ProfileId>> {
        let mut next = self
            .records
            .candidates
            .get(owner.0)?
            .profiles
            .map(|id| id.full(self.records.profiles.owner));
        while let Some(id) = next {
            self.budget.work(1)?;
            let profile = self.records.profiles.get(id.0)?;
            if profile.owner != owner {
                return Err(StoreError::Identity);
            }
            if profile.compiled == compiled {
                return Ok(Some(id));
            }
            next = profile.next;
        }
        Ok(None)
    }
    fn ensure_profile(&mut self, owner: CandidateId, compiled: usize) -> Result<ProfileId> {
        if self.profile_state(owner, compiled)? != ProfileState::Unseen
            || self.records.candidates.get(owner.0)?.complete
        {
            return Err(StoreError::State);
        }
        if let Some(id) = self.find_profile(owner, compiled)? {
            return Ok(id);
        }
        let next = self
            .records
            .candidates
            .get(owner.0)?
            .profiles
            .map(|id| id.full(self.records.profiles.owner));
        let id = ProfileId(self.records.profiles.insert(
            Profile {
                owner,
                compiled,
                changes: None,
                tail: None,
                next,
                error: None,
            },
            &mut self.budget,
            false,
        )?);
        let local = LocalProfileId::from_full(id, self.records.profiles.owner)?;
        self.records.candidates.get_mut(owner.0)?.profiles = Some(local);
        self.budget.usage.live_profiles += 1;
        Ok(id)
    }
    pub(super) fn record_changed_step(
        &mut self,
        owner: CandidateId,
        compiled: usize,
        step: usize,
        output_len: usize,
    ) -> Result<()> {
        self.active(|store| {
            let profile = store.ensure_profile(owner, compiled)?;
            if let Some(tail) = store.records.profiles.get(profile.0)?.tail {
                if store.records.changes.get(tail.0)?.step >= step {
                    return Err(StoreError::State);
                }
            }
            let id = ChangeId(store.records.changes.insert(
                Change {
                    owner: profile,
                    step,
                    output_len,
                    next: None,
                },
                &mut store.budget,
                false,
            )?);
            if let Some(tail) = store.records.profiles.get(profile.0)?.tail {
                store.records.changes.get_mut(tail.0)?.next = Some(id);
            } else {
                store.records.profiles.get_mut(profile.0)?.changes = Some(id);
            }
            store.records.profiles.get_mut(profile.0)?.tail = Some(id);
            store.budget.usage.live_changes += 1;
            Ok(())
        })
    }
    pub(super) fn complete_profile(&mut self, owner: CandidateId, compiled: usize) -> Result<()> {
        self.active(|store| {
            if store.records.candidates.get(owner.0)?.complete {
                return Err(StoreError::State);
            }
            let state = if store.find_profile(owner, compiled)?.is_some() {
                ProfileState::RecordedComplete
            } else {
                ProfileState::ImplicitUnchanged
            };
            store.set_profile_state(owner, compiled, state)
        })
    }
    pub(super) fn record_transform_error(
        &mut self,
        owner: CandidateId,
        compiled: usize,
        error: CheckpointError,
    ) -> Result<()> {
        self.active(|store| {
            let id = store.ensure_profile(owner, compiled)?;
            store.discard_profile_outcomes(owner, compiled)?;
            store.records.profiles.get_mut(id.0)?.error = Some(error);
            store.set_profile_state(owner, compiled, ProfileState::SemanticError)
        })
    }
    fn outcome_insert(
        &mut self,
        owner: CandidateId,
        group: usize,
        stage: usize,
        result: StoredOutcome,
    ) -> Result<OutcomeId> {
        let profile = self
            .records
            .plan()
            .groups
            .get(group)
            .ok_or(StoreError::Identity)?
            .profile;
        if self.profile_state(owner, profile)? != ProfileState::Unseen || self.records.candidates.get(owner.0)?.complete
        {
            return Err(StoreError::State);
        }
        let next = self
            .records
            .candidates
            .get(owner.0)?
            .outcomes
            .map(|id| id.full(self.records.outcomes.owner));
        let id = OutcomeId(self.records.outcomes.insert(
            Outcome {
                owner,
                group,
                stage,
                next,
                result,
            },
            &mut self.budget,
            false,
        )?);
        let local = LocalOutcomeId::from_full(id, self.records.outcomes.owner)?;
        self.records.candidates.get_mut(owner.0)?.outcomes = Some(local);
        self.budget.usage.live_outcomes += 1;
        Ok(id)
    }
    pub(super) fn record_match(
        &mut self,
        owner: CandidateId,
        group: usize,
        stage: usize,
        value: &[u8],
    ) -> Result<OutcomeId> {
        self.active(|store| {
            let lease = store.records.bytes.copy(value, &mut store.budget)?;
            #[cfg(test)]
            measurements::record(Event::MatchedCopyBytes, value.len());
            let id = store.outcome_insert(
                owner,
                group,
                stage,
                StoredOutcome::Match {
                    value: lease,
                    owns_value: true,
                    captures: None,
                    tail: None,
                },
            )?;
            store.budget.usage.matched_bytes = add(store.budget.usage.matched_bytes, value.len())?;
            Ok(id)
        })
    }
    pub(super) fn stage_match_token(&mut self, id: OutcomeId) -> Result<StageMatchToken> {
        self.active(|store| {
            store.budget.work(1)?;
            if !matches!(
                store.records.outcomes.get(id.0)?.result,
                StoredOutcome::Match { owns_value: true, .. }
            ) {
                return Err(StoreError::Identity);
            }
            Ok(StageMatchToken(id))
        })
    }
    pub(super) fn record_shared_match(
        &mut self,
        owner: CandidateId,
        group: usize,
        stage: usize,
        token: StageMatchToken,
    ) -> Result<OutcomeId> {
        self.active(|store| {
            store.budget.work(1)?;
            let source = store.records.outcomes.get(token.0 .0)?;
            let groups = &store.records.plan().groups;
            if source.owner != owner
                || source.stage != stage
                || groups.get(source.group).ok_or(StoreError::Identity)?.profile
                    != groups.get(group).ok_or(StoreError::Identity)?.profile
            {
                return Err(StoreError::Identity);
            }
            let StoredOutcome::Match {
                value,
                owns_value: true,
                ..
            } = source.result
            else {
                return Err(StoreError::Identity);
            };
            store.outcome_insert(
                owner,
                group,
                stage,
                StoredOutcome::Match {
                    value,
                    owns_value: false,
                    captures: None,
                    tail: None,
                },
            )
        })
    }
    pub(super) fn add_capture(&mut self, owner: OutcomeId, key: &[u8], value: &[u8]) -> Result<()> {
        self.add_capture_value(owner, key, Some(value), 0, value.len())
    }
    pub(super) fn add_capture_range(&mut self, owner: OutcomeId, key: &[u8], start: usize, len: usize) -> Result<()> {
        self.add_capture_value(owner, key, None, start, len)
    }
    fn add_capture_value(
        &mut self,
        owner: OutcomeId,
        key: &[u8],
        owned: Option<&[u8]>,
        start: usize,
        len: usize,
    ) -> Result<()> {
        self.active(|store| {
            let outcome = store.records.outcomes.get(owner.0)?;
            let candidate = outcome.owner;
            let profile = store
                .records
                .plan()
                .groups
                .get(outcome.group)
                .ok_or(StoreError::Identity)?
                .profile;
            if store.profile_state(candidate, profile)? != ProfileState::Unseen {
                return Err(StoreError::State);
            }
            let StoredOutcome::Match {
                tail, value: parent, ..
            } = outcome.result
            else {
                return Err(StoreError::State);
            };
            if owned.is_none() {
                store.records.bytes.read(parent)?;
                if add(start, len)? > parent.len {
                    return Err(StoreError::Identity);
                }
            }
            let key_lease = store.records.bytes.copy(key, &mut store.budget)?;
            #[cfg(test)]
            measurements::record(Event::CaptureCopyBytes, key.len());
            let value = if let Some(bytes) = owned {
                let lease = store.records.bytes.copy(bytes, &mut store.budget)?;
                #[cfg(test)]
                measurements::record(Event::CaptureCopyBytes, bytes.len());
                StoredCaptureValue::Owned(lease)
            } else {
                StoredCaptureValue::MatchRange { start, len }
            };
            let id = CaptureId(store.records.captures.insert(
                Capture {
                    owner,
                    key: key_lease,
                    value,
                    next: None,
                },
                &mut store.budget,
                false,
            )?);
            if let Some(tail) = tail {
                store.records.captures.get_mut(tail.0)?.next = Some(id);
            }
            let StoredOutcome::Match {
                captures,
                tail: stored_tail,
                ..
            } = &mut store.records.outcomes.get_mut(owner.0)?.result
            else {
                return Err(StoreError::State);
            };
            if captures.is_none() {
                *captures = Some(id);
            }
            *stored_tail = Some(id);
            store.budget.usage.live_captures += 1;
            store.budget.usage.capture_bytes = add(store.budget.usage.capture_bytes, add(key.len(), len)?)?;
            Ok(())
        })
    }
    pub(super) fn record_semantic_error(
        &mut self,
        owner: CandidateId,
        group: usize,
        stage: usize,
        error: CheckpointError,
    ) -> Result<()> {
        self.active(|store| {
            store.outcome_insert(owner, group, stage, StoredOutcome::Error(error))?;
            Ok(())
        })
    }
    fn free_outcome(&mut self, id: OutcomeId, owner: CandidateId) -> Result<()> {
        if self.records.outcomes.get(id.0)?.owner != owner {
            return Err(StoreError::Identity);
        }
        let outcome = self.records.outcomes.remove(id.0, &mut self.budget)?;
        if let StoredOutcome::Match {
            value,
            owns_value,
            captures,
            ..
        } = outcome.result
        {
            if owns_value {
                self.records.bytes.free(value, &mut self.budget)?;
                self.budget.usage.matched_bytes = sub(self.budget.usage.matched_bytes, value.len)?;
            }
            let mut next = captures;
            while let Some(capture) = next {
                if self.records.captures.get(capture.0)?.owner != id {
                    return Err(StoreError::Identity);
                }
                let capture = self.records.captures.remove(capture.0, &mut self.budget)?;
                next = capture.next;
                self.records.bytes.free(capture.key, &mut self.budget)?;
                if let StoredCaptureValue::Owned(value) = capture.value {
                    self.records.bytes.free(value, &mut self.budget)?;
                }
                self.budget.usage.capture_bytes = sub(
                    self.budget.usage.capture_bytes,
                    add(capture.key.len, capture.value.len())?,
                )?;
                self.budget.usage.live_captures = sub(self.budget.usage.live_captures, 1)?;
            }
        }
        self.budget.usage.live_outcomes = sub(self.budget.usage.live_outcomes, 1)?;
        Ok(())
    }
    fn discard_profile_outcomes(&mut self, owner: CandidateId, profile: usize) -> Result<()> {
        let mut next = self
            .records
            .candidates
            .get(owner.0)?
            .outcomes
            .map(|id| id.full(self.records.outcomes.owner));
        let mut previous: Option<OutcomeId> = None;
        while let Some(id) = next {
            self.budget.work(1)?;
            let outcome = self.records.outcomes.get(id.0)?;
            if outcome.owner != owner {
                return Err(StoreError::Identity);
            }
            next = outcome.next;
            if self
                .records
                .plan()
                .groups
                .get(outcome.group)
                .ok_or(StoreError::Identity)?
                .profile
                == profile
            {
                if let Some(previous) = previous {
                    self.records.outcomes.get_mut(previous.0)?.next = next;
                } else {
                    let local = next
                        .map(|id| LocalOutcomeId::from_full(id, self.records.outcomes.owner))
                        .transpose()?;
                    self.records.candidates.get_mut(owner.0)?.outcomes = local;
                }
                self.free_outcome(id, owner)?;
            } else {
                previous = Some(id);
            }
        }
        Ok(())
    }
    pub(super) fn completion_proof(
        &mut self,
        candidate: CandidateId,
        applicable_profiles: &[usize],
    ) -> Result<CompletionProof> {
        self.active(|store| {
            for &profile in applicable_profiles {
                store.budget.work(1)?;
                store.validate_profile(candidate, profile)?;
            }
            Ok(CompletionProof { candidate })
        })
    }
    pub(super) fn complete_candidate(&mut self, proof: CompletionProof) -> Result<Receipt> {
        self.active(|store| {
            let candidate = store.records.candidates.get_mut(proof.candidate.0)?;
            if candidate.complete {
                return Err(StoreError::State);
            }
            candidate.complete = true;
            Ok(Receipt {
                head: proof.candidate,
                tail: proof.candidate,
            })
        })
    }
    fn validate_profile(&mut self, owner: CandidateId, profile: usize) -> Result<()> {
        match self.profile_state(owner, profile)? {
            ProfileState::Unseen => Err(StoreError::State),
            ProfileState::ImplicitUnchanged => Ok(()),
            state => {
                let id = self.find_profile(owner, profile)?.ok_or(StoreError::Identity)?;
                let stored = self.records.profiles.get(id.0)?;
                if (state == ProfileState::SemanticError) != stored.error.is_some() {
                    return Err(StoreError::Identity);
                }
                Ok(())
            }
        }
    }
    pub(super) fn concat(&mut self, left: Option<Receipt>, right: Option<Receipt>) -> Result<Option<Receipt>> {
        self.active(|store| match (left, right) {
            (Some(left), Some(right)) => {
                store.budget.work(2)?;
                if left.tail == right.head
                    || store.records.candidates.get(left.tail.0)?.next.is_some()
                    || !store.records.candidates.get(left.tail.0)?.complete
                    || !store.records.candidates.get(right.head.0)?.complete
                {
                    return Err(StoreError::Identity);
                }
                store.records.candidates.get(right.tail.0)?;
                store.records.candidates.get(left.head.0)?;
                let local = LocalCandidateId::from_full(right.head, store.records.candidates.owner)?;
                store.records.candidates.get_mut(left.tail.0)?.next = Some(local);
                Ok(Some(Receipt {
                    head: left.head,
                    tail: right.tail,
                }))
            }
            (None, Some(receipt)) | (Some(receipt), None) => {
                store.budget.work(2)?;
                let head = store.records.candidates.get(receipt.head.0)?;
                let tail = store.records.candidates.get(receipt.tail.0)?;
                if !head.complete || !tail.complete || tail.next.is_some() {
                    return Err(StoreError::Identity);
                }
                Ok(Some(receipt))
            }
            (None, None) => Ok(None),
        })
    }
    pub(super) fn discard(&mut self, receipt: Receipt) -> Result<()> {
        self.active(|store| {
            let mut next = Some(receipt.head);
            while let Some(id) = next {
                let candidate = store.records.candidates.remove(id.0, &mut store.budget)?;
                next = candidate.next.map(|id| id.full(store.records.candidates.owner));
                let mut outcomes = candidate.outcomes.map(|id| id.full(store.records.outcomes.owner));
                while let Some(outcome) = outcomes {
                    outcomes = store.records.outcomes.get(outcome.0)?.next;
                    store.free_outcome(outcome, id)?;
                }
                let mut profiles = candidate.profiles.map(|id| id.full(store.records.profiles.owner));
                while let Some(profile) = profiles {
                    if store.records.profiles.get(profile.0)?.owner != id {
                        return Err(StoreError::Identity);
                    }
                    let record = store.records.profiles.remove(profile.0, &mut store.budget)?;
                    profiles = record.next;
                    let mut changes = record.changes;
                    while let Some(change) = changes {
                        if store.records.changes.get(change.0)?.owner != profile {
                            return Err(StoreError::Identity);
                        }
                        changes = store.records.changes.remove(change.0, &mut store.budget)?.next;
                        store.budget.usage.live_changes = sub(store.budget.usage.live_changes, 1)?;
                    }
                    store.budget.usage.live_profiles = sub(store.budget.usage.live_profiles, 1)?;
                }
                store.records.bytes.free(candidate.name, &mut store.budget)?;
                store.records.bytes.free(candidate.states, &mut store.budget)?;
                store.budget.usage.name_bytes = sub(store.budget.usage.name_bytes, candidate.name.len)?;
                store.budget.usage.profile_metadata_bytes =
                    sub(store.budget.usage.profile_metadata_bytes, candidate.states.len)?;
                if let Some(lease) = candidate.deferred {
                    store.records.bytes.free(lease, &mut store.budget)?;
                    store.budget.usage.deferred_bytes = sub(store.budget.usage.deferred_bytes, lease.len)?;
                    store.budget.usage.deferred_entries = sub(store.budget.usage.deferred_entries, 1)?;
                }
                store.budget.usage.live_candidates = sub(store.budget.usage.live_candidates, 1)?;
                if id == receipt.tail {
                    if next.is_some() {
                        return Err(StoreError::Identity);
                    }
                    break;
                }
                if next.is_none() {
                    return Err(StoreError::Identity);
                }
            }
            Ok(())
        })
    }
    pub(super) fn seal(&mut self, receipt: Option<Receipt>, summary: JsonSemanticSummary) -> Result<()> {
        if self.records.domain != InspectionDomain::Json {
            return Err(StoreError::Identity);
        }
        self.seal_summary(receipt, InspectionSummary::Json(summary))
    }
    pub(super) fn seal_xml(&mut self, receipt: Option<Receipt>, summary: XmlSemanticSummary) -> Result<()> {
        if self.records.domain != InspectionDomain::Xml {
            return Err(StoreError::Identity);
        }
        self.seal_summary(receipt, InspectionSummary::Xml(summary))
    }
    fn seal_summary(&mut self, receipt: Option<Receipt>, summary: InspectionSummary) -> Result<()> {
        self.active(|store| {
            let sole_profile = if store.records.domain == InspectionDomain::Json {
                store.records.plan().summary_profile()
            } else {
                None
            };
            store.budget.work(store.records.active_groups.len())?;
            let mut summary_no_hit = !store.records.active_groups.contains(&false)
                && sole_profile.is_some()
                && store.records.profiles.slots.capacity() == 0
                && store.records.changes.slots.capacity() == 0
                && store.records.outcomes.slots.capacity() == 0;
            let demands = store.records.plan().named_demands.len();
            store
                .budget
                .grow(&mut store.records.named_first, demands, false, false)?;
            store.budget.work(demands)?;
            store.records.named_first.resize(demands, None);
            let mut next = receipt.map(|r| r.head);
            let mut last = None;
            let mut count = 0;
            while let Some(id) = next {
                store.budget.work(1)?;
                let candidate = store.records.candidates.get(id.0)?;
                if !candidate.complete {
                    return Err(StoreError::State);
                }
                if summary_no_hit {
                    summary_no_hit = store.profile_state(id, sole_profile.ok_or(StoreError::State)?)?
                        == ProfileState::ImplicitUnchanged;
                }
                if candidate.deferred.is_some() {
                    let name = store.records.bytes.read(candidate.name)?;
                    let (mut low, mut high) = (0, demands);
                    while low < high {
                        store.budget.work(add(name.len(), 1)?)?;
                        let mid = low + (high - low) / 2;
                        let expected = store.records.plan().named_demands[mid]
                            .bytes(&store.records.generation)
                            .map_err(|_| StoreError::Identity)?;
                        match name.cmp(expected) {
                            std::cmp::Ordering::Equal => {
                                if store.records.named_first[mid].is_none() {
                                    store.records.named_first[mid] = Some(id);
                                }
                                break;
                            }
                            std::cmp::Ordering::Less => high = mid,
                            std::cmp::Ordering::Greater => low = mid + 1,
                        }
                    }
                }
                next = candidate.next.map(|id| id.full(store.records.candidates.owner));
                let name = candidate.name;
                let mut outcome_next = candidate.outcomes.map(|id| id.full(store.records.outcomes.owner));
                let mut required = [false; 2];
                while let Some(outcome_id) = outcome_next {
                    store.budget.work(1)?;
                    let outcome = store.records.outcomes.get(outcome_id.0)?;
                    if outcome.owner != id {
                        return Err(StoreError::Identity);
                    }
                    if matches!(outcome.result, StoredOutcome::Match { .. }) {
                        let group = store
                            .records
                            .plan()
                            .groups
                            .get(outcome.group)
                            .ok_or(StoreError::Identity)?;
                        let view = match group.view {
                            JsonView::Value => 0,
                            JsonView::Name => 1,
                        };
                        required[view] = true;
                    }
                    outcome_next = outcome.next;
                }
                let has_name = required.into_iter().any(|required| required);
                let mut pair = QualifiedNamePair::default();
                if has_name {
                    store.budget.grow(&mut store.records.qualified_names, 1, true, false)?;
                }
                for (view, (required, collection)) in required
                    .into_iter()
                    .zip([CollectionKind::Args, CollectionKind::ArgsNames])
                    .enumerate()
                {
                    if !required {
                        continue;
                    }
                    let prefix = if store.records.domain == InspectionDomain::Xml {
                        b"XML".as_slice()
                    } else {
                        collection.as_str().as_bytes()
                    };
                    let named = store.records.candidates.get(id.0)?.kind != CandidateKind::XmlText;
                    store.records.bytes.read(name)?;
                    let separator = usize::from(named);
                    let len = add(add(prefix.len(), separator)?, name.len)?;
                    let value = store.records.bytes.alloc(len, &mut store.budget)?;
                    store.budget.work(len)?;
                    let start = value.offset;
                    let name_start = add(start, add(prefix.len(), separator)?)?;
                    store
                        .records
                        .bytes
                        .bytes
                        .get_mut(start..add(start, prefix.len())?)
                        .ok_or(StoreError::Identity)?
                        .copy_from_slice(prefix);
                    if named {
                        *store
                            .records
                            .bytes
                            .bytes
                            .get_mut(add(start, prefix.len())?)
                            .ok_or(StoreError::Identity)? = b':';
                    }
                    store
                        .records
                        .bytes
                        .bytes
                        .get(name_start..add(name_start, name.len)?)
                        .ok_or(StoreError::Identity)?;
                    store
                        .records
                        .bytes
                        .bytes
                        .copy_within(name.offset..add(name.offset, name.len)?, name_start);
                    pair.values[view] = Some(QualifiedSpan {
                        offset: NonZeroUsize::new(add(value.offset, 1)?).ok_or(StoreError::Arithmetic)?,
                        len,
                    });
                    store.budget.usage.name_bytes = add(store.budget.usage.name_bytes, len)?;
                }
                if has_name {
                    let index =
                        u32::try_from(add(store.records.qualified_names.len(), 1)?).map_err(|_| StoreError::Records)?;
                    let index = NonZeroU32::new(index).ok_or(StoreError::Arithmetic)?;
                    store.budget.work(size_of::<QualifiedNamePair>())?;
                    store.records.qualified_names.push(pair);
                    store.records.candidates.get_mut(id.0)?.qualified_name_pair = Some(index);
                }
                last = Some(id);
                count = add(count, 1)?;
                if count > store.budget.usage.live_candidates {
                    return Err(StoreError::Identity);
                }
            }
            if last != receipt.map(|r| r.tail) || count != store.budget.usage.live_candidates {
                return Err(StoreError::Identity);
            }
            // Mint only after survivor identity/order and sparse name
            // publication have succeeded.
            store.summary_no_hit = summary_no_hit && count == summary.candidate_count();
            store.sealed = receipt;
            store.summary = Some(summary);
            store.terminal = Terminal::Sealed;
            Ok(())
        })
    }
    /// Syntax abandonment releases payloads but must not reset physical work or
    /// peak history before DetectionOnly fallback publication.
    pub(super) fn into_abandoned_budget(self) -> Result<InspectionBudget> {
        if let Terminal::Failed(error) = self.terminal {
            return Err(error);
        }
        if let Some(error) = self.budget.failure {
            return Err(error);
        }
        if !matches!(self.terminal, Terminal::Abandoned) || self.budget.scratch != 0 {
            return Err(StoreError::State);
        }
        Ok(self.budget)
    }
    pub(super) fn into_sealed(self) -> Result<(SealedResults<'generation>, InspectionBudget)> {
        if let Terminal::Failed(error) = self.terminal {
            return Err(error);
        }
        if !matches!(self.terminal, Terminal::Sealed) {
            return Err(StoreError::State);
        }
        Ok((
            SealedResults {
                records: self.records,
                receipt: self.sealed,
                summary: self.summary.ok_or(StoreError::State)?,
                summary_no_hit: self.summary_no_hit,
            },
            self.budget,
        ))
    }
    pub(super) fn admit_scratch(&mut self, required_bytes: usize) -> Result<()> {
        self.active(|store| {
            store.budget.peak(add(
                sub(store.budget.usage.retained_bytes, store.budget.scratch)?,
                required_bytes,
            )?)
        })
    }
    pub(super) fn scratch_capacity(&mut self, actual_bytes: usize) -> Result<()> {
        self.active(|store| {
            let total = add(
                sub(store.budget.usage.retained_bytes, store.budget.scratch)?,
                actual_bytes,
            )?;
            store.budget.peak(total)?;
            store.budget.usage.retained_bytes = total;
            store.budget.scratch = actual_bytes;
            Ok(())
        })
    }
    /// Caller must first drop every externally owned scratch allocation. This
    /// terminal-safe cleanup never clears the saved inspection failure.
    pub(super) fn release_scratch(&mut self) {
        self.budget.release_scratch();
    }
    pub(super) fn charge_work(&mut self, work: usize) -> Result<()> {
        self.active(|store| store.budget.work(work))
    }
    pub(super) fn abandon(&mut self) {
        if matches!(self.terminal, Terminal::Abandoned | Terminal::Failed(_)) {
            return;
        }
        let owner = self.records.owner;
        self.records.candidates = Pool::new(owner);
        self.records.profiles = Pool::new(owner);
        self.records.changes = Pool::new(owner);
        self.records.outcomes = Pool::new(owner);
        self.records.captures = Pool::new(owner);
        self.records.bytes = BytePool::new(owner);
        self.records.active_groups = Vec::new();
        self.records.named_first = Vec::new();
        self.records.qualified_names = Vec::new();
        let previous = self.budget.usage;
        self.budget.usage = StoreUsage {
            retained_bytes: self.budget.scratch,
            peak_retained_bytes: previous.peak_retained_bytes,
            work: previous.work,
            abandoned: previous.abandoned + 1,
            ..StoreUsage::default()
        };
        self.sealed = None;
        self.summary = None;
        self.terminal = Terminal::Abandoned;
    }
}

pub(super) struct CandidateView<'a> {
    pub kind: CandidateKind,
    pub name: &'a [u8],
    pub semantic_len: usize,
    pub deferred: Option<&'a [u8]>,
    pub next: Option<CandidateId>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SelectedOutcome {
    NoMatch,
    MatchOrError(OutcomeId),
    TransformError(CheckpointError),
}
pub(super) struct ProfileView {
    pub record: Option<ProfileId>,
    pub first_change: Option<ChangeId>,
    pub error: Option<CheckpointError>,
}
pub(super) struct ChangeView {
    pub output_len: usize,
    pub next: Option<ChangeId>,
}
pub(super) struct CaptureView {
    pub key: SealedMatchValue,
    pub value: SealedMatchValue,
    pub next: Option<CaptureId>,
}
pub(super) enum OutcomeValue {
    Match {
        value: SealedMatchValue,
        first_capture: Option<CaptureId>,
    },
    Error(CheckpointError),
}
pub(super) struct OutcomeView {
    pub stage: usize,
    pub result: OutcomeValue,
}

impl SealedResults<'_> {
    pub(super) fn domain(&self) -> InspectionDomain {
        self.records.domain
    }
    pub(super) fn json_summary(&self) -> Result<JsonSemanticSummary> {
        match self.summary {
            InspectionSummary::Json(summary) => Ok(summary),
            _ => Err(StoreError::Identity),
        }
    }

    pub(super) fn guard_group(&self, budget: &mut InspectionBudget, group: usize) -> Result<()> {
        budget.checked(self.records.owner, |_| {
            if self.records.group_active(group)? {
                Ok(())
            } else {
                Err(StoreError::State)
            }
        })
    }
    pub(super) fn admits_summary_no_hit(
        &self,
        budget: &InspectionBudget,
        generation: &CompiledGeneration,
        runtime: RuntimeLimits,
        mode: EvaluationMode,
        work: usize,
    ) -> bool {
        self.summary_no_hit
            && mode == EvaluationMode::Accelerated
            && std::ptr::eq(&*self.records.generation, generation)
            && self.records.runtime == runtime
            && self.records.mode == mode
            && budget.admits_work(self.records.owner, work)
    }
    fn sealed_handle(&self, value: Lease) -> Result<SealedMatchValue> {
        self.records.bytes.read(value)?;
        Ok(SealedMatchValue {
            owner: value.owner,
            offset: value.offset,
            len: value.len,
        })
    }
    /// Read-only public observation seam. Runtime execution uses match_value.
    pub(super) fn observe_match_value(&self, value: SealedMatchValue) -> Result<&[u8]> {
        if value.owner != self.records.owner {
            return Err(StoreError::Identity);
        }
        self.records
            .bytes
            .bytes
            .get(value.offset..add(value.offset, value.len)?)
            .ok_or(StoreError::Identity)
    }
    pub(super) fn qualified_name(
        &self,
        budget: &mut InspectionBudget,
        owner: CandidateId,
        collection: CollectionKind,
    ) -> Result<SealedMatchValue> {
        budget.checked(self.records.owner, |budget| {
            budget.work(1)?;
            let candidate = self.records.candidates.get(owner.0)?;
            let index = candidate.qualified_name_pair.ok_or(StoreError::Identity)?;
            let pair = self
                .records
                .qualified_names
                .get(index.get() as usize - 1)
                .ok_or(StoreError::Identity)?;
            let view = match collection {
                CollectionKind::Args if self.records.domain == InspectionDomain::Json => 0,
                CollectionKind::ArgsNames if self.records.domain == InspectionDomain::Json => 1,
                CollectionKind::Xml if self.records.domain == InspectionDomain::Xml => 0,
                _ => return Err(StoreError::Identity),
            };
            let span = pair.values[view].ok_or(StoreError::Identity)?;
            let offset = span.offset.get() - 1;
            self.records
                .bytes
                .bytes
                .get(offset..add(offset, span.len)?)
                .ok_or(StoreError::Identity)?;
            Ok(SealedMatchValue {
                owner: self.records.owner,
                offset,
                len: span.len,
            })
        })
    }

    pub(super) fn named_value(&self, budget: &mut InspectionBudget, demand: usize) -> Result<Option<&[u8]>> {
        budget.checked(self.records.owner, |budget| {
            budget.work(1)?;
            let id = self.records.named_first.get(demand).ok_or(StoreError::Identity)?;
            let Some(id) = id else {
                return Ok(None);
            };
            let candidate = self.records.candidates.get(id.0)?;
            if !candidate.complete {
                return Err(StoreError::State);
            }
            let lease = candidate.deferred.ok_or(StoreError::Identity)?;
            self.records.bytes.read(lease).map(Some)
        })
    }
    pub(super) fn candidate(&self, budget: &mut InspectionBudget, id: CandidateId) -> Result<CandidateView<'_>> {
        budget.checked(self.records.owner, |budget| {
            budget.work(1)?;
            let candidate = self.records.candidates.get(id.0)?;
            if !candidate.complete {
                return Err(StoreError::State);
            }
            Ok(CandidateView {
                kind: candidate.kind,
                name: self.records.bytes.read(candidate.name)?,
                semantic_len: candidate.semantic_len,
                deferred: candidate
                    .deferred
                    .map(|lease| self.records.bytes.read(lease))
                    .transpose()?,
                next: candidate.next.map(|id| id.full(self.records.candidates.owner)),
            })
        })
    }
    pub(super) fn profile(
        &self,
        budget: &mut InspectionBudget,
        owner: CandidateId,
        compiled: usize,
    ) -> Result<ProfileView> {
        budget.checked(self.records.owner, |budget| {
            budget.work(1)?;
            if compiled >= self.records.plan().profiles.len() {
                return Err(StoreError::Identity);
            }
            let candidate = self.records.candidates.get(owner.0)?;
            if !candidate.complete {
                return Err(StoreError::State);
            }
            let states = self.records.bytes.read(candidate.states)?;
            let state = match (states[compiled / 4] >> (2 * (compiled % 4))) & 3 {
                0 => return Err(StoreError::State),
                1 => ProfileState::ImplicitUnchanged,
                2 => ProfileState::RecordedComplete,
                _ => ProfileState::SemanticError,
            };
            if state == ProfileState::ImplicitUnchanged {
                return Ok(ProfileView {
                    record: None,
                    first_change: None,
                    error: None,
                });
            }
            let mut next = candidate.profiles.map(|id| id.full(self.records.profiles.owner));
            while let Some(id) = next {
                budget.work(1)?;
                let profile = self.records.profiles.get(id.0)?;
                if profile.owner != owner {
                    return Err(StoreError::Identity);
                }
                if profile.compiled == compiled {
                    if (state == ProfileState::SemanticError) != profile.error.is_some() {
                        return Err(StoreError::Identity);
                    }
                    return Ok(ProfileView {
                        record: Some(id),
                        first_change: profile.changes,
                        error: profile.error,
                    });
                }
                next = profile.next;
            }
            Err(StoreError::Identity)
        })
    }
    pub(super) fn change(
        &self,
        budget: &mut InspectionBudget,
        owner: CandidateId,
        profile: ProfileId,
        id: ChangeId,
    ) -> Result<ChangeView> {
        budget.checked(self.records.owner, |budget| {
            budget.work(1)?;
            self.records.candidates.get(owner.0)?;
            if self.records.profiles.get(profile.0)?.owner != owner {
                return Err(StoreError::Identity);
            }
            let value = self.records.changes.get(id.0)?;
            if value.owner != profile {
                return Err(StoreError::Identity);
            }
            Ok(ChangeView {
                output_len: value.output_len,
                next: value.next,
            })
        })
    }
    /// The canonical selector provides the applicable group. No lookup can infer
    /// no-match from an unseen profile or a transform-error profile.
    pub(super) fn lookup_selected(
        &self,
        budget: &mut InspectionBudget,
        owner: CandidateId,
        group: usize,
        generation: &CompiledGeneration,
        runtime: RuntimeLimits,
        mode: EvaluationMode,
    ) -> Result<(SelectedOutcome, ProfileView)> {
        budget.checked(self.records.owner, |budget| {
            budget.work(1)?;
            if !std::ptr::eq(&*self.records.generation, generation)
                || self.records.runtime != runtime
                || self.records.mode != mode
            {
                return Err(StoreError::Identity);
            }
            if !self.records.group_active(group)? {
                return Err(StoreError::State);
            }
            let compiled = self
                .records
                .plan()
                .groups
                .get(group)
                .ok_or(StoreError::Identity)?
                .profile;
            let profile = self.profile(budget, owner, compiled)?;
            if let Some(error) = profile.error {
                return Ok((SelectedOutcome::TransformError(error), profile));
            }
            let mut next = self
                .records
                .candidates
                .get(owner.0)?
                .outcomes
                .map(|id| id.full(self.records.outcomes.owner));
            while let Some(id) = next {
                budget.work(1)?;
                let outcome = self.records.outcomes.get(id.0)?;
                if outcome.owner != owner {
                    return Err(StoreError::Identity);
                }
                if outcome.group == group {
                    return Ok((SelectedOutcome::MatchOrError(id), profile));
                }
                next = outcome.next;
            }
            Ok((SelectedOutcome::NoMatch, profile))
        })
    }
    pub(super) fn outcome(
        &self,
        budget: &mut InspectionBudget,
        owner: CandidateId,
        id: OutcomeId,
    ) -> Result<OutcomeView> {
        budget.checked(self.records.owner, |budget| {
            budget.work(1)?;
            self.records.candidates.get(owner.0)?;
            let outcome = self.records.outcomes.get(id.0)?;
            if outcome.owner != owner {
                return Err(StoreError::Identity);
            }
            let result = match outcome.result {
                StoredOutcome::Match { value, captures, .. } => {
                    self.records.bytes.read(value)?;
                    OutcomeValue::Match {
                        value: SealedMatchValue {
                            owner: value.owner,
                            offset: value.offset,
                            len: value.len,
                        },
                        first_capture: captures,
                    }
                }
                StoredOutcome::Error(error) => OutcomeValue::Error(error),
            };
            Ok(OutcomeView {
                stage: outcome.stage,
                result,
            })
        })
    }
    pub(super) fn match_value(&self, budget: &mut InspectionBudget, value: SealedMatchValue) -> Result<&[u8]> {
        budget.checked(self.records.owner, |budget| {
            budget.work(1)?;
            self.observe_match_value(value)
        })
    }
    pub(super) fn capture(
        &self,
        budget: &mut InspectionBudget,
        owner: CandidateId,
        outcome: OutcomeId,
        id: CaptureId,
    ) -> Result<CaptureView> {
        budget.checked(self.records.owner, |budget| {
            budget.work(1)?;
            self.records.candidates.get(owner.0)?;
            let outcome_record = self.records.outcomes.get(outcome.0)?;
            if outcome_record.owner != owner {
                return Err(StoreError::Identity);
            }
            let capture = self.records.captures.get(id.0)?;
            if capture.owner != outcome {
                return Err(StoreError::Identity);
            }
            Ok(CaptureView {
                key: self.sealed_handle(capture.key)?,
                value: match capture.value {
                    StoredCaptureValue::Owned(value) => self.sealed_handle(value)?,
                    StoredCaptureValue::MatchRange { start, len } => {
                        let StoredOutcome::Match { value, .. } = outcome_record.result else {
                            return Err(StoreError::Identity);
                        };
                        self.records.bytes.read(value)?;
                        if add(start, len)? > value.len {
                            return Err(StoreError::Identity);
                        }
                        SealedMatchValue {
                            owner: value.owner,
                            offset: add(value.offset, start)?,
                            len,
                        }
                    }
                },
                next: capture.next,
            })
        })
    }
}

#[cfg(test)]
#[path = "../../tests/unit/seclang/json_inspection_test.rs"]
mod tests;
