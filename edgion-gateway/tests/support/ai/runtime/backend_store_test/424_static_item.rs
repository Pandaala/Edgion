#[cfg(test)]
static GLOBAL_AI_BACKEND_TEST_LOCK: LazyLock<parking_lot::Mutex<()>> = LazyLock::new(|| parking_lot::Mutex::new(()));
