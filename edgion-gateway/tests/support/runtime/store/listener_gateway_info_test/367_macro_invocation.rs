#[cfg(test)]
thread_local! {
    static TEST_INDEX_OVERRIDES: std::cell::RefCell<HashMap<ListenerKey, Arc<GatewayInfoIndex>>> =
        std::cell::RefCell::new(HashMap::new());
}
