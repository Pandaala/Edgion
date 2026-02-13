//! Common utilities shared across EdgionPlugins
//!
//! Provides shared infrastructure for plugins that need to make HTTP requests
//! to external services (e.g., ForwardAuth, OPA, Webhook).

pub mod auth_common;
pub mod http_client;
pub mod jwt_common;

pub use auth_common::{resolve_claim_path, send_auth_error_response, set_claims_headers, Claims};
pub use http_client::{get_http_client, is_hop_by_hop, HOP_BY_HOP_HEADERS};
pub use jwt_common::{
    default_allowed_algs, jwk_matches_alg, map_jwt_decode_error, resolve_algorithm_policy, select_jwk,
    validate_token_alg, JwkSelectError,
};
