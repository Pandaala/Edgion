//! Rotates a gateway through its list of controller addresses to find the
//! leader (spec §7.2/§7.5). The list is shuffled once per gateway at
//! construction so a fleet does not hammer the same dead node first after a
//! failover; backoff applies only after a full failed lap.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use rand::seq::SliceRandom;
use rand::Rng;

pub struct EndpointRotator {
    addrs: Vec<String>,
    cursor: AtomicUsize,
    lap_failures: AtomicUsize,
}

impl EndpointRotator {
    pub fn new(mut addrs: Vec<String>) -> Self {
        addrs.shuffle(&mut rand::rng());
        Self {
            addrs,
            cursor: AtomicUsize::new(0),
            lap_failures: AtomicUsize::new(0),
        }
    }

    /// True when there is at most one address — callers skip rotation entirely,
    /// which is the K8s zero-impact guarantee (spec §10.5 S5/S6).
    pub fn is_single(&self) -> bool {
        self.addrs.len() <= 1
    }

    pub fn current(&self) -> String {
        self.addrs[self.cursor.load(Ordering::Acquire) % self.addrs.len()].clone()
    }

    /// Move to the next address (a no-op address-wise for a one-element list),
    /// recording one failure toward the current lap.
    pub fn advance(&self) -> String {
        if self.addrs.len() > 1 {
            self.cursor.fetch_add(1, Ordering::AcqRel);
        }
        self.lap_failures.fetch_add(1, Ordering::AcqRel);
        self.current()
    }

    /// Reset the lap-failure count ("stream established AND first event
    /// received" — spec §7.5).
    pub fn note_success(&self) {
        self.lap_failures.store(0, Ordering::Release);
    }

    /// True once every address has failed since the last success.
    pub fn completed_full_lap(&self) -> bool {
        self.lap_failures.load(Ordering::Acquire) >= self.addrs.len()
    }

    /// Jittered backoff, only meaningful after a full failed lap (spec §7.5):
    /// grows with the number of completed failed laps, capped at 30s.
    pub async fn backoff_after_lap(&self) {
        let laps = (self.lap_failures.load(Ordering::Acquire) / self.addrs.len().max(1)).min(6) as u32;
        let cap_ms = (500u64 << laps).min(30_000);
        let ms = rand::rng().random_range(cap_ms / 2..=cap_ms);
        tokio::time::sleep(Duration::from_millis(ms)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_address_never_rotates() {
        let r = EndpointRotator::new(vec!["a:1".into()]);
        assert!(r.is_single());
        assert_eq!(r.current(), "a:1");
        assert_eq!(r.advance(), "a:1"); // one element: advance is a no-op address-wise
    }

    #[test]
    fn multi_address_cycles_and_detects_full_lap() {
        let r = EndpointRotator::new(vec!["a:1".into(), "b:1".into()]);
        let first = r.current();
        let second = r.advance();
        assert_ne!(first, second);
        let third = r.advance(); // wrapped
        assert_eq!(third, first);
        assert!(r.completed_full_lap(), "two advances over two addrs = a full lap");
        r.note_success();
        assert!(!r.completed_full_lap());
    }

    #[test]
    fn shuffle_preserves_the_address_set() {
        let addrs: Vec<String> = (0..8).map(|i| format!("addr-{i}:1")).collect();
        let r = EndpointRotator::new(addrs.clone());
        let mut seen: Vec<String> = (0..8).map(|_| r.advance()).collect();
        seen.sort();
        let mut expect = addrs;
        expect.sort();
        assert_eq!(seen, expect, "one full lap visits every address exactly once");
    }
}
