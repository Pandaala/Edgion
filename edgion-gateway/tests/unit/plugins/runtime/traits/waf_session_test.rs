use super::*;
use crate::plugins::runtime::traits::session::{MockPluginSession, PluginSession};

#[test]
fn feature_mock_keeps_short_borrows_and_explicit_phase_behavior() {
    let owner = Arc::new(());
    let mut waf = MockWafSessionCapability::new();
    waf.expect_claim_phase()
        .times(1)
        .returning(|_, _| WafPhaseClaim::Execute);
    waf.expect_take_transaction().times(1).returning(|_| None);
    let mut session = MockPluginSession::new();
    session
        .expect_waf()
        .return_var(Box::new(waf) as Box<dyn WafSessionCapability>);
    session.expect_get_method().times(1).return_const("GET".to_owned());

    assert_eq!(
        session.waf().claim_phase(&owner, WafPhase::Request),
        WafPhaseClaim::Execute
    );
    assert_eq!(session.get_method(), "GET");
    assert!(session.waf().take_transaction(&owner).is_none());
}
