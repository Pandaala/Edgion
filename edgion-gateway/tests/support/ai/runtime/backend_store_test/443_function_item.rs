#[cfg(test)]
pub fn lock_global_ai_backend_store_for_test() -> GlobalAiBackendTestGuard {
    let lock = GLOBAL_AI_BACKEND_TEST_LOCK.lock();
    GLOBAL_EDGION_BACKEND_STORE.replace_all(&HashMap::new());
    GlobalAiBackendTestGuard { _lock: lock }
}
