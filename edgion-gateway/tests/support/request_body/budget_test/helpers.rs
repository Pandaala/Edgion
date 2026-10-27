tokio::task_local! {
    pub(crate) static TEST_BUDGET: Arc<StorageBudget>;
}

pub(crate) fn process_budget() -> Arc<StorageBudget> {
    TEST_BUDGET.try_with(Arc::clone).unwrap_or_else(|_| shared_process_budget())
}

impl StorageBudget {
    pub(crate) fn usage(&self) -> [u64; 4] {
        [&self.memory, &self.disk, &self.captures, &self.files].map(|counter| counter.used.load(Ordering::Acquire))
    }
}
