#[cfg(test)]
impl Drop for GlobalAiBackendTestGuard {
    fn drop(&mut self) {
        GLOBAL_EDGION_BACKEND_STORE.replace_all(&HashMap::new());
    }
}
