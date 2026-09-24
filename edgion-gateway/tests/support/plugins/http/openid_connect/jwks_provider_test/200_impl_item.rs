#[cfg(test)]
impl ScriptedHttp {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: ScriptMutex::new(std::collections::HashMap::new()),
        })
    }

    pub fn origin() -> &'static str {
        TEST_IDP_ORIGIN
    }

    pub fn jwks_url() -> String {
        format!("{TEST_IDP_ORIGIN}{JWKS_PATH}")
    }

    pub fn enqueue_http(&self, path: &str, status: u16, body: impl Into<Vec<u8>>) {
        self.push(
            path,
            ScriptedEvent::Http {
                status,
                body: body.into(),
            },
            false,
        );
    }

    pub fn enqueue_http_repeat(&self, path: &str, status: u16, body: impl Into<Vec<u8>>) {
        self.push(
            path,
            ScriptedEvent::Http {
                status,
                body: body.into(),
            },
            true,
        );
    }

    pub fn enqueue_timeout(&self, path: &str) {
        self.push(path, ScriptedEvent::Timeout, false);
    }

    pub fn enqueue_transport(&self, path: &str) {
        self.push(path, ScriptedEvent::Transport, false);
    }

    pub fn mount_ok_discovery(&self) {
        let body = serde_json::json!({
            "issuer": TEST_IDP_ORIGIN,
            "jwks_uri": Self::jwks_url(),
        })
        .to_string()
        .into_bytes();
        self.enqueue_http_repeat(DISCOVERY_PATH, 200, body);
    }

    pub fn mount_live_jwks(
        &self,
        fail: Arc<std::sync::atomic::AtomicBool>,
        keys: Arc<std::sync::atomic::AtomicUsize>,
        hits: Arc<std::sync::atomic::AtomicUsize>,
    ) {
        let mut routes = self.inner.lock();
        let route = routes.entry(JWKS_PATH.to_string()).or_default();
        route.live_jwks = Some(LiveJwks { fail, keys, hits });
    }

    pub fn hits(&self, path: &str) -> usize {
        self.inner.lock().get(path).map(|route| route.hits).unwrap_or(0)
    }

    fn push(&self, path: &str, event: ScriptedEvent, repeat: bool) {
        let mut routes = self.inner.lock();
        let route = routes.entry(path.to_string()).or_default();
        if repeat {
            route.repeat = Some(event);
        } else {
            route.queue.push_back(event);
        }
    }

    fn route_key(url: &str) -> &'static str {
        if url.contains(DISCOVERY_PATH) {
            DISCOVERY_PATH
        } else if url.contains(JWKS_PATH) {
            JWKS_PATH
        } else {
            panic!("OIDC unit test scripted HTTP has no route for {url}");
        }
    }

    fn exchange(&self, url: &str, operation: OidcOperation) -> Result<reqwest::Response, OidcDependencyError> {
        let path = Self::route_key(url);
        let event = {
            let mut routes = self.inner.lock();
            let route = routes.entry(path.to_string()).or_default();
            route.hits += 1;
            if let Some(live) = &route.live_jwks {
                live.hits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if live.fail.load(std::sync::atomic::Ordering::SeqCst) {
                    ScriptedEvent::Http {
                        status: 503,
                        body: Vec::new(),
                    }
                } else {
                    let n = live.keys.load(std::sync::atomic::Ordering::SeqCst);
                    let keys = (0..n)
                        .map(|i| format!(r#"{{"kty":"oct","kid":"k{i}","k":"AAAAAA","alg":"HS256"}}"#))
                        .collect::<Vec<_>>()
                        .join(",");
                    ScriptedEvent::Http {
                        status: 200,
                        body: format!(r#"{{"keys":[{keys}]}}"#).into_bytes(),
                    }
                }
            } else if let Some(event) = route.queue.pop_front() {
                event
            } else if let Some(event) = route.repeat.clone() {
                event
            } else {
                panic!("OIDC unit test scripted HTTP exhausted for {path} ({url})");
            }
        };
        match event {
            ScriptedEvent::Http { status, body } => Ok(http::Response::builder()
                .status(status)
                .body(body)
                .unwrap_or_else(|_| panic!("scripted OIDC HTTP status {status} must be valid"))
                .into()),
            ScriptedEvent::Timeout => Err(OidcDependencyError::new(operation, OidcDependencyFailure::Timeout)),
            ScriptedEvent::Transport => Err(OidcDependencyError::new(operation, OidcDependencyFailure::Transport)),
        }
    }
}
