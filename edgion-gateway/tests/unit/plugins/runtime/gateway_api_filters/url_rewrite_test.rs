use super::*;
use crate::plugins::runtime::traits::session::MockPluginSession;
use edgion_resources::resources::{HTTPPathModifier, HTTPPathModifierType};

#[tokio::test]
async fn run_request_sets_exact_upstream_path_and_raw_query() {
    let filter = URLRewriteFilter::try_new(&HTTPURLRewriteFilter {
        hostname: None,
        path: Some(HTTPPathModifier {
            modifier_type: HTTPPathModifierType::ReplaceFullPath,
            replace_full_path: Some("/v2/orders".to_string()),
            replace_prefix_match: None,
        }),
    })
    .unwrap();
    let mut session = MockPluginSession::new();
    session
        .expect_get_query()
        .return_const(Some("q=hello%20world&next=%2Forders%3Fstatus%3Dopen".to_string()));
    session.expect_set_upstream_uri().times(1).returning(|uri| {
        assert_eq!(uri, "/v2/orders?q=hello%20world&next=%2Forders%3Fstatus%3Dopen");
        Ok(())
    });

    let mut log = PluginLog::new("test");
    assert_eq!(
        filter.run_request(&mut session, &mut log).await,
        PluginRunningResult::GoodNext
    );
}

#[tokio::test]
async fn upstream_uri_setter_failure_fails_closed() {
    let filter = URLRewriteFilter::try_new(&HTTPURLRewriteFilter {
        hostname: None,
        path: Some(HTTPPathModifier {
            modifier_type: HTTPPathModifierType::ReplaceFullPath,
            replace_full_path: Some("/v2/orders".to_string()),
            replace_prefix_match: None,
        }),
    })
    .unwrap();
    let mut session = MockPluginSession::new();
    session.expect_get_query().return_const(None);
    session
        .expect_set_upstream_uri()
        .times(1)
        .returning(|_| Err(Box::new(std::io::Error::other("setter failed"))));

    let mut log = PluginLog::new("test");
    assert!(matches!(
        filter.run_request(&mut session, &mut log).await,
        PluginRunningResult::ErrResponse {
            status: 500,
            body: None
        }
    ));
}

#[tokio::test]
async fn prefix_rewrite_without_matched_prefix_fails_closed() {
    let filter = URLRewriteFilter::try_new(&HTTPURLRewriteFilter {
        hostname: None,
        path: Some(HTTPPathModifier {
            modifier_type: HTTPPathModifierType::ReplacePrefixMatch,
            replace_full_path: None,
            replace_prefix_match: Some("/v2".to_string()),
        }),
    })
    .unwrap();
    let mut session = MockPluginSession::new();
    session.expect_get_path().return_const("/api/orders".to_string());
    session.expect_ctx().return_const(crate::ctx::EdgionHttpContext::new());
    session.expect_set_upstream_uri().times(0);

    let mut log = PluginLog::new("test");
    assert!(matches!(
        filter.run_request(&mut session, &mut log).await,
        PluginRunningResult::ErrResponse {
            status: 500,
            body: None
        }
    ));
}
