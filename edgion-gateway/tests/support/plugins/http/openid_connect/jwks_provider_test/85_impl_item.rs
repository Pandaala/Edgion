#[cfg(test)]
impl DiscoveryDoc for LeanDiscoveryDocument {
    fn issuer(&self) -> &str {
        &self.issuer
    }
    fn jwks_uri(&self) -> &str {
        &self.jwks_uri
    }
}
