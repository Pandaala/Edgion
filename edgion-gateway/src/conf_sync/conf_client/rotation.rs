//! Candidate traversal is separate from the installed channel. Only actual
//! remote failures advance traversal; local preparation errors retain a candidate.

use std::time::Duration;

use rand::seq::SliceRandom;
use rand::Rng;

pub(crate) struct EndpointRotator {
    addrs: Vec<String>,
    candidate: usize,
    searching: bool,
    failures: usize,
    revision: u64,
    paused_laps: usize,
}

impl EndpointRotator {
    pub(crate) fn new(mut addrs: Vec<String>) -> Self {
        addrs.shuffle(&mut rand::rng());
        Self {
            addrs,
            candidate: 0,
            searching: false,
            failures: 0,
            revision: 0,
            paused_laps: 0,
        }
    }

    pub(crate) fn is_single(&self) -> bool {
        self.addrs.len() <= 1
    }

    pub(crate) fn address(&self, index: usize) -> &str {
        &self.addrs[index]
    }

    /// Count the failed installed attempt once, even when local preparation retries.
    pub(crate) fn begin_search(&mut self, installed: usize) {
        if !self.searching {
            self.searching = true;
            self.candidate = installed;
            self.note_failure();
        }
    }

    pub(crate) fn candidate(&self) -> (usize, u64) {
        (self.candidate, self.revision)
    }

    pub(crate) fn matches(&self, index: usize, revision: u64) -> bool {
        self.searching && self.candidate == index && self.revision == revision
    }

    pub(crate) fn note_failure(&mut self) {
        self.failures = self.failures.saturating_add(1);
        self.candidate = (self.candidate + 1) % self.addrs.len();
        self.revision = self.revision.wrapping_add(1);
    }

    /// A successful unary probe does not prove the metadata stream works.
    pub(crate) fn installed(&mut self) {
        self.searching = false;
        self.revision = self.revision.wrapping_add(1);
    }

    pub(crate) fn note_success(&mut self) {
        self.failures = 0;
        self.paused_laps = 0;
        self.installed();
    }

    pub(crate) fn take_lap_delay(&mut self) -> Option<Duration> {
        let laps = self.failures / self.addrs.len();
        if laps <= self.paused_laps {
            return None;
        }
        self.paused_laps = laps;
        let cap = Self::lap_cap_ms(laps);
        Some(Duration::from_millis(rand::rng().random_range(cap / 2..=cap)))
    }

    fn lap_cap_ms(laps: usize) -> u64 {
        (500_u64 << laps.min(6)).min(30_000)
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/conf_sync/conf_client/rotation_test.rs"]
mod tests;
