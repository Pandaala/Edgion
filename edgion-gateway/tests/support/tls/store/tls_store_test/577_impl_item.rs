#[cfg(test)]
impl TlsStore {
    fn is_rejected_active(&self, key: &str) -> bool {
        self.matcher
            .data
            .load()
            .entries
            .get(key)
            .map(|e| e.rejected_active)
            .unwrap_or(false)
    }
}
