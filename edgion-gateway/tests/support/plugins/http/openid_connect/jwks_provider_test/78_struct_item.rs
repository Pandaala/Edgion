/// Minimal discovery doc test fixture — only the two universal fields.
#[cfg(test)]
#[derive(Debug, Deserialize)]
pub struct LeanDiscoveryDocument {
    pub issuer: String,
    pub jwks_uri: String,
}
