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
mod tests {
    use super::*;

    #[test]
    fn local_failure_retains_candidate_and_count() {
        let mut r = EndpointRotator::new(vec!["a".into(), "b".into()]);
        r.begin_search(0);
        let candidate = r.candidate();
        r.begin_search(0);
        assert_eq!(r.candidate(), candidate);
        assert_eq!(r.failures, 1);
        assert_eq!(candidate.0, 1);
        assert!(r.take_lap_delay().is_none());
    }

    #[test]
    fn two_failed_laps_back_off_once_each_and_cap() {
        let mut r = EndpointRotator::new(vec!["a".into(), "b".into()]);
        r.begin_search(0);
        r.note_failure();
        assert_eq!(r.candidate().0, 0);
        let first = r.take_lap_delay().unwrap();
        assert!((Duration::from_millis(500)..=Duration::from_millis(1000)).contains(&first));
        assert!(r.take_lap_delay().is_none());
        r.note_failure();
        assert!(r.take_lap_delay().is_none());
        r.note_failure();
        let second = r.take_lap_delay().unwrap();
        assert!((Duration::from_millis(1000)..=Duration::from_millis(2000)).contains(&second));
        assert_eq!(EndpointRotator::lap_cap_ms(100), 30_000);
        r.note_success();
        assert_eq!(r.failures, 0);
        assert!(r.take_lap_delay().is_none());
    }

    #[test]
    fn successful_probe_does_not_reset_meta_failures() {
        let mut r = EndpointRotator::new(vec!["a".into(), "b".into()]);
        r.begin_search(0);
        let ticket = r.candidate();
        r.installed();
        assert!(!r.matches(ticket.0, ticket.1));
        r.begin_search(1);
        assert!(r.take_lap_delay().is_some(), "failed metadata completed a lap");
    }

    #[test]
    fn shuffle_preserves_addresses_and_single_stays_single() {
        let addrs: Vec<String> = (0..8).map(|i| format!("addr-{i}:1")).collect();
        let r = EndpointRotator::new(addrs.clone());
        let mut seen = r.addrs;
        seen.sort();
        assert_eq!(seen, addrs);
        let r = EndpointRotator::new(vec!["a".into()]);
        assert!(r.is_single());
        assert_eq!(r.address(0), "a");
    }
}
