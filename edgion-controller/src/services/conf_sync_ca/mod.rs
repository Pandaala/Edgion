//! Controller-embedded internal CA for conf_sync mTLS (auto-provisioning v0).
//! Promoted from the one-shot `edgion-cli/src/certs` generator; persists the CA
//! key and can re-sign leaves from a loaded root.

mod ca;
pub mod cert_util;
pub mod identity_sync;
mod provision;
pub mod rotation;
mod secret_io;
pub use ca::{Ca, LeafPem};
pub use identity_sync::start_identity_sync;
pub use provision::{leaf_needs_rewrite, provision, server_sans_for, ServerIdentityPem};
pub use secret_io::{build_secret, ensure_ca, load_ca, read_leaf, write_leaf, CA_SECRET, LEAF_SECRET};
