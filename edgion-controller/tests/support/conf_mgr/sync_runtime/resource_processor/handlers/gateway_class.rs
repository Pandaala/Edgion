use super::*;

#[cfg(test)]
impl Default for GatewayClassHandler {
    fn default() -> Self {
        Self::new(GatewayClassOwnership::new(DEFAULT_CONTROLLER_NAME.to_string()))
    }
}
