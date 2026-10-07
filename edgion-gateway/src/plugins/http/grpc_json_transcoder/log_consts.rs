//! Access-log substrings asserted by shared-topology integration tests.
pub const OWNER_CONFLICT: &str = "conversion owner conflict";
pub const INVALID_STATUS: &str = "terminal status invalid";
pub const RPC_FAILURE: &str = "upstream status failed";
pub const COMPLETED: &str = "exchange completed";

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/http/grpc_json_transcoder/log_consts_test.rs"]
mod tests;
