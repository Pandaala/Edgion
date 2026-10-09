//! Exact process-instance selection, not resource identity or Grant authority.

/// Bounded errors retain startup validation order without echoing environment values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GatewayProcessLocatorError {
    UidRequiresScope,
    InvalidScope,
}

/// Borrowed process scope and optional exact instance UID.
///
/// This finite grammar deliberately does not trim, default, or DNS-validate fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GatewayProcessLocator<'a> {
    raw_scope: &'a str,
    namespace: &'a str,
    name: &'a str,
    uid: Option<&'a str>,
}

impl<'a> GatewayProcessLocator<'a> {
    pub fn from_process(
        scope: Option<&'a str>,
        uid: Option<&'a str>,
    ) -> Result<Option<Self>, GatewayProcessLocatorError> {
        if uid.is_some_and(|uid| uid.is_empty() || scope.is_none()) {
            return Err(GatewayProcessLocatorError::UidRequiresScope);
        }
        let Some(raw_scope) = scope else {
            return Ok(None);
        };
        let (namespace, name) = raw_scope
            .split_once('/')
            .filter(|(namespace, name)| !namespace.is_empty() && !name.is_empty() && !name.contains('/'))
            .ok_or(GatewayProcessLocatorError::InvalidScope)?;
        Ok(Some(Self {
            raw_scope,
            namespace,
            name,
            uid,
        }))
    }

    pub fn raw_scope(self) -> &'a str {
        self.raw_scope
    }
    pub fn namespace(self) -> &'a str {
        self.namespace
    }
    pub fn name(self) -> &'a str {
        self.name
    }
    pub fn uid(self) -> Option<&'a str> {
        self.uid
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/resources/refs/gateway_process_locator_test.rs"]
mod tests;
