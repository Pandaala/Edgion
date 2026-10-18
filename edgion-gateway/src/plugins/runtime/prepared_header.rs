//! Fixed header values prepared before plugin execution.

use http::{header::InvalidHeaderValue, HeaderValue};

pub(crate) struct PreparedHeader {
    name: String,
    value: HeaderValue,
}

impl PreparedHeader {
    pub(crate) fn new(name: String, value: &str) -> Result<Self, InvalidHeaderValue> {
        let value = HeaderValue::from_bytes(value.as_bytes())?;
        // Promote freshly allocated Bytes storage on the cold path so even the
        // first request clone only increments the shared reference.
        let shared_value = value.clone();
        drop(value);
        Ok(Self {
            name,
            value: shared_value,
        })
    }

    #[inline]
    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    #[inline]
    pub(crate) fn value(&self) -> &HeaderValue {
        &self.value
    }
}
