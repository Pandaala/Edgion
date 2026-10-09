use super::*;

#[test]
fn process_locator_preserves_raw_bytes_and_borrows_components() {
    assert_eq!(GatewayProcessLocator::from_process(None, None), Ok(None));
    for scope in ["infra/edge", "Infra_Name/Edge-1", " infra / edge ", "café/π", " / "] {
        for uid in [None, Some("uid-1"), Some(" ")] {
            let locator = GatewayProcessLocator::from_process(Some(scope), uid).unwrap().unwrap();
            let (namespace, name) = scope.split_once('/').unwrap();
            assert_eq!(locator.raw_scope(), scope);
            assert_eq!(locator.namespace(), namespace);
            assert_eq!(locator.name(), name);
            assert_eq!(locator.uid(), uid);
            assert!(std::ptr::eq(locator.raw_scope().as_ptr(), scope.as_ptr()));
            assert!(std::ptr::eq(locator.namespace().as_ptr(), scope.as_ptr()));
            assert!(std::ptr::eq(locator.name().as_ptr(), name.as_ptr()));
        }
    }
}

#[test]
fn process_locator_rejects_malformed_scope_and_checks_uid_first() {
    for scope in ["", "bare", "/edge", "infra/", "infra/edge/extra", "//"] {
        for uid in [None, Some("uid-1")] {
            assert_eq!(
                GatewayProcessLocator::from_process(Some(scope), uid),
                Err(GatewayProcessLocatorError::InvalidScope)
            );
        }
        assert_eq!(
            GatewayProcessLocator::from_process(Some(scope), Some("")),
            Err(GatewayProcessLocatorError::UidRequiresScope)
        );
    }
    for uid in ["", "uid-1", " "] {
        assert_eq!(
            GatewayProcessLocator::from_process(None, Some(uid)),
            Err(GatewayProcessLocatorError::UidRequiresScope)
        );
    }
    assert_eq!(
        GatewayProcessLocator::from_process(Some("infra/edge"), Some("")),
        Err(GatewayProcessLocatorError::UidRequiresScope)
    );
}
