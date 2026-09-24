#[cfg(test)]
struct LiveJwks {
    fail: Arc<std::sync::atomic::AtomicBool>,
    keys: Arc<std::sync::atomic::AtomicUsize>,
    hits: Arc<std::sync::atomic::AtomicUsize>,
}
