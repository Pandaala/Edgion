//! LdapAuth placeholder for builds without the `ldap` feature.
//!
//! TLS validation always fails, so the plugin runtime records every LdapAuth
//! entry as invalid and the route fails closed. No filter is ever constructed.

use std::convert::Infallible;

use edgion_common::link_sys_core::outbound_tls::{GlobalOutboundTlsSnapshot, ValidatedLocalTlsSelection};
use edgion_resources::resources::refs::plugin_fields::BoundPluginEntry;

use crate::plugins::runtime::RequestFilter;

const LDAP_NOT_COMPILED: &str = "LdapAuth is not compiled into this gateway build (cargo feature `ldap`)";

pub struct LdapAuth {
    _never: Infallible,
}

impl LdapAuth {
    pub fn create(
        _entry: &BoundPluginEntry<'_>,
        _defaults: &GlobalOutboundTlsSnapshot,
    ) -> Option<Box<dyn RequestFilter>> {
        None
    }

    pub(crate) fn validate_tls_entry<'a>(
        _entry: &'a BoundPluginEntry<'_>,
        _defaults: &GlobalOutboundTlsSnapshot,
    ) -> Result<Option<ValidatedLocalTlsSelection<'a>>, String> {
        Err(LDAP_NOT_COMPILED.to_string())
    }
}
