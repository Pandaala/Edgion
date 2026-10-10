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
    use edgion_resources::resources::refs::acme_fields::AcmeRefs;
    let refs = AcmeRefs::from_acme(acme);
    refs.account().token().validated().map_err(|_| {
        "privateKeySecretRef must reference a core Secret (group='', kind='Secret') with valid source/name/namespace"
    })?;
    if let Some(field) = refs.eab() {
        field.token().validated().map_err(|_| "externalAccountBinding.keySecretRef must reference a core Secret (group='', kind='Secret') with valid source/name/namespace")?;
    }
    Ok(())
}
