//! Controller-embedded internal CA for conf_sync mTLS (auto-provisioning v0).
//! Promoted from the one-shot `edgion-cli/src/certs` generator; persists the CA
//! key and can re-sign leaves from a loaded root.

mod ca;
mod provision;
mod secret_io;
pub use ca::{Ca, LeafPem};
pub use provision::{provision, server_sans_for, ServerIdentityPem};
pub use secret_io::{build_secret, ensure_ca, write_leaf, CA_SECRET, LEAF_SECRET};
