//! Publish-path call counters. Publication runs inline on the caller thread,
//! so counters are thread-local: parallel tests publishing through other store
//! instances or the global store cannot perturb a test's counts.

use std::cell::Cell;
use std::sync::atomic::Ordering;
use std::thread::LocalKey;

pub struct ThreadCounter(&'static LocalKey<Cell<usize>>);

impl ThreadCounter {
    pub fn fetch_add(&self, value: usize, _order: Ordering) -> usize {
        self.0.with(|count| count.replace(count.get() + value))
    }

    pub fn load(&self, _order: Ordering) -> usize {
        self.0.with(Cell::get)
    }

    fn reset(&self) {
        self.0.with(|count| count.set(0));
    }
}

thread_local! {
    static PREPARE_POLICY: Cell<usize> = const { Cell::new(0) };
    static FULL_INDEX: Cell<usize> = const { Cell::new(0) };
    static SIDE_EFFECT: Cell<usize> = const { Cell::new(0) };
}

pub static PREPARE_POLICY_CALLS: ThreadCounter = ThreadCounter(&PREPARE_POLICY);
pub static FULL_INDEX_BUILDS: ThreadCounter = ThreadCounter(&FULL_INDEX);
pub static SIDE_EFFECT_CALLS: ThreadCounter = ThreadCounter(&SIDE_EFFECT);

pub fn reset() {
    PREPARE_POLICY_CALLS.reset();
    FULL_INDEX_BUILDS.reset();
    SIDE_EFFECT_CALLS.reset();
}
