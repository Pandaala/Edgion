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
mod tests {
    use super::*;

    #[test]
    fn identity_follows_gateway_controller_contract() {
        for name in ["example.com/controller", "example.com/Team/a:1", "a/x"] {
            assert!(validate_controller_name(name).is_ok(), "{name}");
        }
        for name in [
            "",
            " ",
            "controller",
            "Example.com/a",
            "example.com/",
            "a..b/x",
            "-a/x",
            "a/x y",
            "a/x?y",
        ] {
            assert!(validate_controller_name(name).is_err(), "{name}");
        }
        assert!(validate_controller_name(&format!("a/{}", "x".repeat(251))).is_ok());
        assert!(validate_controller_name(&format!("a/{}", "x".repeat(252))).is_err());
    }

    #[test]
    fn automatic_identity_is_stable_and_separates_deployments() {
        let resolve = |ns, deployment| resolve_kubernetes_controller_name(None, Some(ns), Some(deployment)).unwrap();
        assert_eq!(
            resolve("system", "controller-a"),
            "edgion.io/gateway-controller/system/controller-a"
        );
        assert_ne!(resolve("system", "controller-a"), resolve("system", "controller-b"));
        assert_ne!(resolve("system", "controller-a"), resolve("other", "controller-a"));
        assert_eq!(
            resolve_kubernetes_controller_name(Some("example.com/custom"), None, None).unwrap(),
            "example.com/custom"
        );
    }

    #[test]
    fn missing_or_invalid_identity_never_falls_back() {
        assert!(require_controller_name(None).is_err());
        assert!(require_controller_name(Some(" ")).is_err());
        assert!(resolve_kubernetes_controller_name(Some(""), Some("system"), Some("controller")).is_err());
        for (ns, deployment) in [
            (None, Some("a")),
            (Some("a"), None),
            (Some(""), Some("a")),
            (Some("a"), Some("a/b")),
            (Some("a.b"), Some("a")),
        ] {
            assert!(resolve_kubernetes_controller_name(None, ns, deployment).is_err());
        }
        assert!(resolve_kubernetes_controller_name(None, Some("ns"), Some(&"a".repeat(253))).is_err());
        let oversized_deployment = ["a".repeat(63), "b".repeat(63), "c".repeat(63), "d".repeat(40)].join(".");
        assert!(resolve_kubernetes_controller_name(None, Some("ns"), Some(&oversized_deployment)).is_err());
    }

    #[test]
    fn deserialization_preserves_explicit_empty_identity() {
        let omitted: super::super::KubernetesConfig = serde_yaml::from_str("{}").unwrap();
        let empty: super::super::KubernetesConfig = serde_yaml::from_str("controller_name: ''").unwrap();
        assert_eq!(omitted.controller_name, None);
        assert_eq!(empty.controller_name.as_deref(), Some(""));
    }
}
