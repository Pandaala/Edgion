#[cfg(test)]
tokio::task_local! {
    pub(crate) static STORAGE_IO_TEST_HOOK: Arc<dyn Fn(StorageIoOperation) + Send + Sync>;
}
