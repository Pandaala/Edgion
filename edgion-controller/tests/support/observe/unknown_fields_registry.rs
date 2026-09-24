/// Test-only module-level mutex, mirroring `status_metrics::TEST_MUTEX`:
/// serializes tests that mutate the global `sources_slot()` against parallel
/// `cargo test` threads in the same process.
#[cfg(test)]
pub(super) static TEST_MUTEX: parking_lot::Mutex<()> = parking_lot::Mutex::new(());
