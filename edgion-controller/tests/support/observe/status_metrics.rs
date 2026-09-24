/// Test-only module-level mutex. All tests in this file that mutate the
/// global `snapshot_slot()` or `renderers_slot()` must lock this before
/// touching state, to serialize otherwise parallel cargo test threads
/// running in the same process.
#[cfg(test)]
pub(super) static TEST_MUTEX: parking_lot::Mutex<()> = parking_lot::Mutex::new(());
