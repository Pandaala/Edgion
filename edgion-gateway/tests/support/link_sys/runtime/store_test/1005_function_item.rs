#[cfg(all(test, feature = "otlp"))]
pub(crate) fn seed_published_otlp(key: String, client: Arc<OtlpLinkClient>) {
    let runner = PersistentRunner::<OtlpApply>::spawn_with_published(key.clone(), client);
    RUNTIMES.install(&key, Some(RuntimeEntry::new(ProviderRuntime::Otlp(runner))));
}
