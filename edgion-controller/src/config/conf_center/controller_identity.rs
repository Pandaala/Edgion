//! Startup-only Gateway API controller identity validation and resolution.

/// Validate the domain-prefixed path contract of GatewayController in Gateway API v1.5.
pub fn validate_controller_name(name: &str) -> anyhow::Result<()> {
    let valid = name.len() <= 253
        && name.split_once('/').is_some_and(|(domain, path)| {
            !domain.is_empty()
                && domain.split('.').all(|label| {
                    !label.is_empty()
                        && label.as_bytes().first().is_some_and(u8::is_ascii_alphanumeric)
                        && label.as_bytes().last().is_some_and(u8::is_ascii_alphanumeric)
                        && label
                            .bytes()
                            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
                })
                && !path.is_empty()
                && path
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"/-._~%!$&'()*+,;=:".contains(&c))
        });
    anyhow::ensure!(
        valid,
        "conf_center.controller_name must be a Gateway API domain-prefixed path of at most 253 characters"
    );
    Ok(())
}

pub fn require_controller_name(name: Option<&str>) -> anyhow::Result<String> {
    let name = name.ok_or_else(|| anyhow::anyhow!("conf_center.controller_name is required outside Kubernetes"))?;
    validate_controller_name(name)?;
    Ok(name.to_string())
}

pub fn resolve_kubernetes_controller_name(
    configured: Option<&str>,
    namespace: Option<&str>,
    deployment: Option<&str>,
) -> anyhow::Result<String> {
    if let Some(name) = configured {
        validate_controller_name(name)?;
        return Ok(name.to_string());
    }
    let namespace = namespace
        .ok_or_else(|| anyhow::anyhow!("POD_NAMESPACE is required when conf_center.controller_name is omitted"))?;
    let deployment = deployment.ok_or_else(|| {
        anyhow::anyhow!("EDGION_CONTROLLER_DEPLOYMENT_NAME is required when conf_center.controller_name is omitted")
    })?;
    let valid_label = |label: &str| {
        !label.is_empty()
            && label.len() <= 63
            && label.as_bytes().first().is_some_and(u8::is_ascii_alphanumeric)
            && label.as_bytes().last().is_some_and(u8::is_ascii_alphanumeric)
            && label
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
    };
    anyhow::ensure!(
        valid_label(namespace),
        "POD_NAMESPACE must be a valid Kubernetes namespace name"
    );
    anyhow::ensure!(
        deployment.len() <= 253 && deployment.split('.').all(valid_label),
        "EDGION_CONTROLLER_DEPLOYMENT_NAME must be a valid Kubernetes Deployment name"
    );
    let name = format!("edgion.io/gateway-controller/{namespace}/{deployment}");
    validate_controller_name(&name)?;
    Ok(name)
}

#[cfg(test)]
#[path = "../../../tests/unit/config/conf_center/controller_identity_test.rs"]
mod tests;
