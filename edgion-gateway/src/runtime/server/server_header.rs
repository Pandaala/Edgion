use bytes::Bytes;
use http::{HeaderName, HeaderValue};
use pingora_http::ResponseHeader;

/// One pre-parsed header entry.
///
/// `name` is used for case-insensitive `remove_header` lookup; `case_name`
/// preserves the original casing for the wire (pingora's titled-name
/// fast-list only covers ~12 common headers, so anything else — e.g. HSTS —
/// would otherwise round-trip through `HeaderName::as_str()` and get
/// lowercased).
#[derive(Clone, Debug)]
struct Entry {
    name: HeaderName,
    case_name: Bytes,
    value: HeaderValue,
}

#[derive(Clone, Debug)]
pub struct ServerHeaderOpts {
    headers: Vec<Entry>,
    enable: bool,
}

const HSTS_KEY: &str = "Strict-Transport-Security";

impl Default for ServerHeaderOpts {
    fn default() -> Self {
        let mut opts = Self {
            headers: Vec::new(),
            enable: true,
        };
        opts.upsert("Server", "Edgion");
        opts.upsert(HSTS_KEY, "max-age=63072000; includeSubDomains; preload");
        opts
    }
}

impl ServerHeaderOpts {
    pub fn remove_hsts(&mut self) {
        self.remove(HSTS_KEY);
    }

    /// Apply configured headers to response.
    ///
    /// `Bytes` and `HeaderValue` clones here are refcount bumps. Pingora's
    /// `insert_header` still re-derives an internal `HeaderName` from the
    /// bytes — cheap enum dispatch for standard headers, including both
    /// Server and HSTS configured here. Genuinely custom names may allocate
    /// during this conversion.
    pub fn apply_to_response(&self, response: &mut ResponseHeader) {
        if self.enable {
            for entry in &self.headers {
                response.remove_header(&entry.name);
                let _ = response.insert_header(entry.case_name.clone(), entry.value.clone());
            }
        }
    }

    fn upsert(&mut self, key: &str, value: &str) {
        let Ok(name) = HeaderName::try_from(key) else {
            return;
        };
        let Ok(val) = HeaderValue::try_from(value) else {
            return;
        };
        match self.headers.iter_mut().find(|e| e.name == name) {
            // Match `HashMap::insert` semantics: existing key (and its case)
            // is preserved; only the value is replaced.
            Some(slot) => slot.value = val,
            None => {
                let case_name = Bytes::copy_from_slice(key.as_bytes());
                self.headers.push(Entry {
                    name,
                    case_name,
                    value: val,
                });
            }
        }
    }

    fn remove(&mut self, key: &str) {
        if let Ok(name) = HeaderName::try_from(key) {
            self.headers.retain(|e| e.name != name);
        }
    }
}
