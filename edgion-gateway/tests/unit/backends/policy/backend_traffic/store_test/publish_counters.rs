use std::sync::atomic::{AtomicUsize, Ordering};

pub static PREPARE_POLICY_CALLS: AtomicUsize = AtomicUsize::new(0);
pub static FULL_INDEX_BUILDS: AtomicUsize = AtomicUsize::new(0);
pub static SIDE_EFFECT_CALLS: AtomicUsize = AtomicUsize::new(0);

pub fn reset() {
    PREPARE_POLICY_CALLS.store(0, Ordering::SeqCst);
    FULL_INDEX_BUILDS.store(0, Ordering::SeqCst);
    SIDE_EFFECT_CALLS.store(0, Ordering::SeqCst);
}
