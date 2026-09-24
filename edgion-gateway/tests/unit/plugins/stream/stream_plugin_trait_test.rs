use super::StreamLayer;

#[test]
fn stream_layer_metric_labels_are_stable() {
    assert_eq!(StreamLayer::Stage1.as_metric_label(), "stage1");
    assert_eq!(StreamLayer::Tcp.as_metric_label(), "tcp");
}
