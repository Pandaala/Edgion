use crate::types::constants::annotations::edgion::HEALTH_CHECK;
use crate::types::resources::health_check::{ActiveHealthCheckConfig, ServiceHealthCheck};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;

/// Parse `edgion.io/health-check` annotation from K8s object metadata.
pub fn parse_health_check_annotation(meta: &ObjectMeta) -> Option<ActiveHealthCheckConfig> {
    let annotations = meta.annotations.as_ref()?;
    let yaml_str = annotations.get(HEALTH_CHECK)?;

    match serde_yaml::from_str::<ServiceHealthCheck>(yaml_str) {
        Ok(config) => {
            if let Some(err) = config.get_validation_error() {
                tracing::warn!(
                    resource = %meta.name.as_deref().unwrap_or(""),
                    error = %err,
                    "Invalid health check annotation, ignoring"
                );
                return None;
            }
            config.active
        }
        Err(e) => {
            tracing::warn!(
                resource = %meta.name.as_deref().unwrap_or(""),
                error = %e,
                "Failed to parse health check annotation"
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn meta_with_annotation(value: Option<&str>) -> ObjectMeta {
        let mut annotations = BTreeMap::new();
        if let Some(v) = value {
            annotations.insert(HEALTH_CHECK.to_string(), v.to_string());
        }
        ObjectMeta {
            name: Some("test".to_string()),
            annotations: if annotations.is_empty() {
                None
            } else {
                Some(annotations)
            },
            ..Default::default()
        }
    }

    #[test]
    fn test_parse_valid_annotation() {
        let meta = meta_with_annotation(Some(
            r#"
active:
  type: http
  path: /healthz
"#,
        ));
        let cfg = parse_health_check_annotation(&meta);
        assert!(cfg.is_some());
    }

    #[test]
    fn test_parse_missing_annotation() {
        let meta = meta_with_annotation(None);
        let cfg = parse_health_check_annotation(&meta);
        assert!(cfg.is_none());
    }

    #[test]
    fn test_parse_invalid_yaml() {
        let meta = meta_with_annotation(Some("active: [invalid"));
        let cfg = parse_health_check_annotation(&meta);
        assert!(cfg.is_none());
    }

    #[test]
    fn test_parse_valid_without_active() {
        let meta = meta_with_annotation(Some("{}"));
        let cfg = parse_health_check_annotation(&meta);
        assert!(cfg.is_none());
    }
}
