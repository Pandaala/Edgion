use super::*;
use edgion_resources::output::LinkSysRef;

#[test]
fn test_es_log_sender_name_returns_key() {
    let r = LinkSysRef {
        namespace: "default".to_string(),
        name: "es-main".to_string(),
    };
    let sender = EsLogSender::new(r);
    assert_eq!(sender.name(), "default/es-main");
}

#[tokio::test]
async fn test_es_log_sender_healthy_false_when_not_registered() {
    let r = LinkSysRef {
        namespace: "default".to_string(),
        name: "nonexistent".to_string(),
    };
    let sender = EsLogSender::new(r);
    assert!(!sender.healthy());
}
