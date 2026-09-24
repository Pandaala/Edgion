use super::*;
#[test]
fn configured_namespace_sources_are_fixed_and_deduplicated() {
    assert!(matches!(
        NamespaceWatchMode::from_namespaces(Vec::new()),
        NamespaceWatchMode::AllNamespaces
    ));
    assert!(
        matches!(NamespaceWatchMode::from_namespaces(vec!["a".into(),"a".into()]),NamespaceWatchMode::SingleNamespace(ns) if ns=="a")
    );
    assert!(
        matches!(NamespaceWatchMode::from_namespaces(vec!["b".into(),"a".into(),"b".into()]),NamespaceWatchMode::MultipleNamespaces(ns) if ns==vec!["a","b"])
    );
}
