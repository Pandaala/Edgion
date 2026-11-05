//! Bounded Center-only publication state, independent of Gateway configuration sync.
pub mod projection;
pub(crate) mod publication;

use parking_lot::Mutex;
use projection::{Record, SourceKey};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::Arc,
};
use tokio::sync::watch;

const MAX_ROWS: usize = 4096;
const MAX_LIVE_BYTES: usize = 4 * 1024 * 1024;
const MAX_RECORD_BYTES: usize = 64 * 1024;
const MAX_EVENTS: usize = 1024;
const MAX_HISTORY_BYTES: usize = 4 * 1024 * 1024;
const MAX_FAILURE_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreError {
    StaleGeneration,
    TooOld,
    FutureVersion,
    Projection,
    Capacity,
}

#[derive(Debug, Clone)]
pub struct Change {
    pub version: u64,
    pub key: SourceKey,
    pub record: Option<Arc<Record>>,
}
#[derive(Debug)]
pub struct Snapshot {
    pub generation: String,
    pub version: u64,
    pub records: Vec<Arc<Record>>,
}
struct Stored {
    record: Arc<Record>,
    bytes: usize,
}
struct State {
    generation: String,
    version: u64,
    history_floor: u64,
    rows: BTreeMap<SourceKey, Stored>,
    bytes: usize,
    history: VecDeque<(Change, usize)>,
    history_bytes: usize,
    fault: Option<StoreError>,
    failures: BTreeMap<SourceKey, StoreError>,
    failure_bytes: usize,
}
impl Default for State {
    fn default() -> Self {
        Self {
            generation: uuid::Uuid::new_v4().to_string(),
            version: 0,
            history_floor: 0,
            rows: BTreeMap::new(),
            bytes: 0,
            history: VecDeque::new(),
            history_bytes: 0,
            fault: None,
            failures: BTreeMap::new(),
            failure_bytes: 0,
        }
    }
}
impl State {
    fn generation(&self, generation: &str) -> Result<(), StoreError> {
        if self.generation == generation {
            Ok(())
        } else {
            Err(StoreError::StaleGeneration)
        }
    }
    fn healthy(&self) -> Result<(), StoreError> {
        self.fault
            .or_else(|| self.failures.values().next().copied())
            .map_or(Ok(()), Err)
    }
    fn source_failure(&mut self, key: SourceKey, error: StoreError) {
        let bytes = key.budget_bytes() + 64;
        if !self.failures.contains_key(&key) {
            if self.failures.len() == MAX_ROWS || self.failure_bytes + bytes > MAX_FAILURE_BYTES {
                // Failure tracking itself is exhausted. Never forget a missing source
                // and subsequently advertise a complete view. Runtime reset is required.
                self.fault = Some(StoreError::Capacity);
                return;
            }
            self.failure_bytes += bytes;
        }
        self.failures.insert(key, error);
    }
    fn clear_failure(&mut self, key: &SourceKey) -> bool {
        if let Some((stored_key, _)) = self.failures.remove_entry(key) {
            self.failure_bytes -= stored_key.budget_bytes() + 64;
            true
        } else {
            false
        }
    }
    fn change(&mut self, key: SourceKey, record: Option<Arc<Record>>, bytes: usize) {
        self.version += 1;
        self.history.push_back((
            Change {
                version: self.version,
                key,
                record,
            },
            bytes,
        ));
        self.history_bytes += bytes;
        while self.history.len() > MAX_EVENTS || self.history_bytes > MAX_HISTORY_BYTES {
            if let Some((old, size)) = self.history.pop_front() {
                self.history_floor = old.version;
                self.history_bytes -= size;
            }
        }
    }
}

pub struct CenterWatchStore {
    state: Mutex<State>,
    changed: watch::Sender<()>,
}
impl Default for CenterWatchStore {
    fn default() -> Self {
        Self {
            state: Mutex::new(State::default()),
            changed: watch::channel(()).0,
        }
    }
}
impl CenterWatchStore {
    pub fn generation(&self) -> String {
        self.state.lock().generation.clone()
    }
    pub fn subscribe(&self) -> watch::Receiver<()> {
        // Receivers use changed()/has_changed(); the unit value carries no state.
        // Release any borrowed watch value before calling back into this store.
        self.changed.subscribe()
    }
    pub fn reset(&self) {
        *self.state.lock() = State::default();
        self.changed.send_replace(());
    }
    pub fn fail(&self, generation: &str, error: StoreError) -> Result<(), StoreError> {
        let mut state = self.state.lock();
        state.generation(generation)?;
        // Only publication errors may poison a generation, never a reader cursor.
        if !matches!(error, StoreError::Projection | StoreError::Capacity) {
            return Err(error);
        }
        state.fault = Some(error);
        drop(state);
        self.changed.send_replace(());
        Err(error)
    }
    pub fn fail_source(&self, generation: &str, key: SourceKey, error: StoreError) -> Result<(), StoreError> {
        let mut state = self.state.lock();
        state.generation(generation)?;
        if !matches!(error, StoreError::Projection | StoreError::Capacity) {
            return Err(error);
        }
        state.source_failure(key, error);
        drop(state);
        self.changed.send_replace(());
        Err(error)
    }
    /// A transport retry can enqueue these keys through the existing processors.
    /// Workers never requeue themselves on a failed projection.
    pub fn failed_keys(&self, generation: &str) -> Result<Vec<SourceKey>, StoreError> {
        let state = self.state.lock();
        state.generation(generation)?;
        Ok(state.failures.keys().cloned().collect())
    }
    pub fn upsert(&self, generation: &str, record: Record) -> Result<(), StoreError> {
        let mut state = self.state.lock();
        state.generation(generation)?;
        if let Some(error) = state.fault {
            return Err(error);
        }
        let key = record.key().clone();
        let bytes = record.budget_bytes() + key.budget_bytes();
        let old = state.rows.get(&key);
        if old.is_some_and(|old| *old.record == record) {
            let recovered = state.clear_failure(&key);
            drop(state);
            if recovered {
                self.changed.send_replace(());
            }
            return Ok(());
        }
        let next_bytes = state.bytes - old.map_or(0, |old| old.bytes) + bytes;
        if bytes > MAX_RECORD_BYTES || next_bytes > MAX_LIVE_BYTES || (old.is_none() && state.rows.len() == MAX_ROWS) {
            state.source_failure(key, StoreError::Capacity);
            drop(state);
            self.changed.send_replace(());
            return Err(StoreError::Capacity);
        }
        let record = Arc::new(record);
        state.rows.insert(
            key.clone(),
            Stored {
                record: record.clone(),
                bytes,
            },
        );
        state.bytes = next_bytes;
        state.clear_failure(&key);
        state.change(key, Some(record), bytes);
        drop(state);
        self.changed.send_replace(());
        Ok(())
    }
    pub fn delete(&self, generation: &str, key: &SourceKey) -> Result<(), StoreError> {
        let mut state = self.state.lock();
        state.generation(generation)?;
        let mut changed = state.clear_failure(key);
        if let Some(old) = state.rows.remove(key) {
            state.bytes -= old.bytes;
            state.change(key.clone(), None, key.budget_bytes());
            changed = true;
        }
        drop(state);
        if changed {
            self.changed.send_replace(());
        }
        Ok(())
    }
    pub fn snapshot(&self) -> Result<Snapshot, StoreError> {
        let state = self.state.lock();
        state.healthy()?;
        Ok(Snapshot {
            generation: state.generation.clone(),
            version: state.version,
            records: state.rows.values().map(|row| row.record.clone()).collect(),
        })
    }
    pub fn events_since(&self, generation: &str, version: u64) -> Result<(u64, Vec<Change>), StoreError> {
        let state = self.state.lock();
        state.generation(generation)?;
        state.healthy()?;
        if version < state.history_floor {
            return Err(StoreError::TooOld);
        }
        if version > state.version {
            return Err(StoreError::FutureVersion);
        }
        Ok((
            state.version,
            state
                .history
                .iter()
                .filter(|(change, _)| change.version > version)
                .map(|(change, _)| change.clone())
                .collect(),
        ))
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/fed_sync/center_watch/mod_test.rs"]
mod tests;
