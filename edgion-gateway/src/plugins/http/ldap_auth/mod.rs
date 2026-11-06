#[cfg(feature = "ldap")]
mod plugin;
#[cfg(not(feature = "ldap"))]
#[path = "plugin_disabled.rs"]
mod plugin;

pub use plugin::LdapAuth;
