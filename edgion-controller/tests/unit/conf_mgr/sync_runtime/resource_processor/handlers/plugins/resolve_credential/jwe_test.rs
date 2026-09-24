use super::*;
use edgion_resources::prelude_resources::EdgionPluginsSpec;
use edgion_resources::resources::edgion_plugins::{JweDecryptConfig, RequestFilterEntry, ResolvedJweCredential};
use edgion_resources::resources::gateway::SecretObjectReference;

fn resource_with(resolved_credential: ResolvedJweCredential) -> EdgionPlugins {
    let mut resource = EdgionPlugins::new("jwe", EdgionPluginsSpec::default());
    resource.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::JweDecrypt(
        JweDecryptConfig {
            secret_ref: Some(SecretObjectReference {
                group: None,
                kind: None,
                name: "jwe-secret".to_string(),
                namespace: None,
            }),
            resolved_credential: Some(resolved_credential),
            ..Default::default()
        },
    ))]);
    resource
}

#[test]
fn status_reports_present_secret_missing_required_key() {
    let errors = status_errors(&resource_with(ResolvedJweCredential { secret: None }));
    assert_eq!(errors, vec![ResolvedRefsError::MissingKeys { keys: vec!["secret"] }]);
}

#[test]
fn status_reports_invalid_key_length_without_secret_material() {
    let private = b"private-short-key";
    let errors = status_errors(&resource_with(ResolvedJweCredential {
        secret: Some(STANDARD.encode(private)),
    }));
    assert_eq!(errors.len(), 1);
    let message = errors[0].message();
    assert!(message.contains("decoded key must be 32 bytes"));
    assert!(!message.contains(std::str::from_utf8(private).expect("test key is valid UTF-8")));
}

#[test]
fn status_ignores_disabled_jwe_with_unusable_material() {
    let mut resource = resource_with(ResolvedJweCredential {
        secret: Some(STANDARD.encode(b"too-short")),
    });
    resource.spec.request_plugins.as_mut().unwrap()[0].enable = false;
    assert!(status_errors(&resource).is_empty());
}
