#[cfg(test)]
pub(crate) fn install_test_webhook(key: &str, client: Option<Arc<WebhookLinkClient>>) {
    RUNTIMES.install(
        key,
        client.map(|client| RuntimeEntry::new(ProviderRuntime::Webhook(client))),
    );
}
