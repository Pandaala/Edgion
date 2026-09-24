/// In-memory IdP HTTP script for unit tests. Compiled only under `cfg(test)`;
/// production builds keep the canonical reqwest client with no extra field or
/// branch on the request path.
#[cfg(test)]
pub struct ScriptedHttp {
    inner: ScriptMutex<std::collections::HashMap<String, ScriptedRoute>>,
}
