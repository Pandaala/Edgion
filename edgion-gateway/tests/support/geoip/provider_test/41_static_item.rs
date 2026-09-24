/// Serializes tests that touch the process-global `GeoIpProvider::global()`.
///
/// `GeoIpProvider::global()` is a single `LazyLock` shared by every test in
/// this process (Cargo runs a crate's tests as parallel threads in one
/// process). Tests that load the fixture into the global and tests that
/// assert the global is empty would otherwise flake against each other; take
/// this lock around any test body that touches `GeoIpProvider::global()`.
#[cfg(test)]
pub static GEO_TEST_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());
