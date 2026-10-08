//! Canonical, runtime-independent resource reference semantics.
//!
//! Owning field adapters choose defaults and supported targets. Identity and
//! exact comparison are shared; authorization and resource stores belong to
//! their consuming runtimes.

pub mod condition_fields;
pub mod egc_tls_fields;
pub mod extension_ref;
pub mod gateway_class_fields;
pub(super) mod grants;
mod identity;
pub mod ip_list_fields;
pub mod link_sys_fields;
pub mod outbound_tls;
mod owned;
mod parent_identity;
pub mod plugin_fields;
pub mod plugin_target;
pub mod policy_fields;
pub mod route_fields;
pub mod source_contract;
pub mod targets;
mod template_fields;
pub mod waf_fields;

pub use grants::GrantQuery;
pub use identity::{GroupKind, ObjectIdentity, ReferenceError, ReferenceSource, ResourceScope};
pub use owned::{BoundFieldReference, FrozenFieldReference, ReferenceToken};
pub use parent_identity::{ParentRefIdentity, ParentStatusIdentity};
pub use template_fields::TemplateSecretReference;
