use super::*;

#[test]
fn asserted_tokens_remain_in_their_owning_sources() {
    let guard = include_str!("../../../../../src/plugins/http/graphql_guard/mod.rs");
    let common = include_str!("../../../../../src/plugins/http/common/graphql_request.rs");
    for token in [SHAPE, MEASURE] {
        assert!(guard.contains(token));
    }
    for token in [COST, COST_HEADER] {
        assert!(common.contains(token));
    }
}
