use std::collections::HashMap;
use pingora_http::ResponseHeader;

#[derive(Clone, Debug)]
pub struct ServerHeaderOpts {
    headers: HashMap<String, String>,
    enable: bool,
}

const HSTS_KEY: &str = "Strict-Transport-Security";
impl Default for ServerHeaderOpts {
    fn default() -> Self {
        let mut headers = HashMap::new();
        headers.insert("Server".to_owned(), "Edgion".to_owned());
        headers.insert(HSTS_KEY.to_owned(), "max-age=63072000; includeSubDomains; preload".to_owned());
        Self {
            headers,
            enable: true,
        }
    }
}
impl ServerHeaderOpts {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_header<S: Into<String>>(mut self, key: S, value: S) {
        self.headers.insert(key.into(), value.into());
    }

    pub fn set_strict_transport_security(&mut self, value: impl Into<String>) {
        self.headers.insert(HSTS_KEY.to_owned(), value.into());
    }

    pub fn has_headers(&self, h: &str) -> bool {
        self.headers.contains_key(h)
    }

    pub fn enable(&mut self, enable: bool) {
        self.enable = enable;
    }

    /// Apply configured headers to response
    pub fn apply_to_response(&self, response: &mut ResponseHeader) {
        if self.enable {
            for (key, value) in &self.headers {
                // Remove existing header first to avoid duplicates
                response.remove_header(key);
                // Insert custom header (clone strings to satisfy 'static lifetime requirement)
                let _ = response.insert_header(key.clone(), value.clone());
            }
        }
    }
}