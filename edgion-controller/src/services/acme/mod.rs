//! ACME Controller-side modules
//!
//! - `acme_client` - ACME protocol client wrapping instant-acme
//! - `service` - Background orchestrator for certificate issuance/renewal

use edgion_resources::resources::edgion_acme::EdgionAcme;

pub mod acme_client;
mod secret_io;
pub mod service;

pub use service::{notify_resource_changed, start_acme_service, stop_acme_service, trigger_resource};

/// ACME supports only core Secret targets, with Gateway API reference defaults.
/// Keep admission and every credential consumer on the same target contract.
pub(crate) fn validate_secret_references(acme: &EdgionAcme) -> Result<(), &'static str> {
    let references = std::iter::once((
        &acme.spec.private_key_secret_ref,
        "privateKeySecretRef must reference a core Secret (group='', kind='Secret')",
    ))
    .chain(acme.spec.external_account_binding.as_ref().map(|binding| {
        (
            &binding.key_secret_ref,
            "externalAccountBinding.keySecretRef must reference a core Secret (group='', kind='Secret')",
        )
    }));
    for (reference, message) in references {
        if reference.group.as_deref().is_some_and(|group| !group.is_empty())
            || reference.kind.as_deref().is_some_and(|kind| kind != "Secret")
        {
            return Err(message);
        }
    }
    Ok(())
}
