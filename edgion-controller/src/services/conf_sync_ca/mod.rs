//! Controller-embedded internal CA for conf_sync mTLS (auto-provisioning v0).
//! Promoted from the one-shot `edgion-cli/src/certs` generator; persists the CA
//! key and can re-sign leaves from a loaded root.

mod ca;
pub mod cert_util;
mod provision;
pub mod rotation;
mod secret_io;
pub use ca::{Ca, LeafPem};
pub use provision::{leaf_needs_rewrite, provision, server_sans_for, ServerIdentityPem};
pub use secret_io::{build_secret, ensure_ca, read_leaf, write_ca, write_leaf, CA_SECRET, LEAF_SECRET};
