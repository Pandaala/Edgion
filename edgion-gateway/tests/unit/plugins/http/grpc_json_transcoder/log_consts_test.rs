use super::*;

#[test]
fn terminal_log_contract_matches_transport_source() {
    let source = include_str!("../../../../../src/plugins/http/grpc_json_transcoder/transport.rs");
    for token in [OWNER_CONFLICT, INVALID_STATUS, RPC_FAILURE, COMPLETED] {
        assert!(source.contains(token), "missing terminal log contract: {token}");
    }
}
