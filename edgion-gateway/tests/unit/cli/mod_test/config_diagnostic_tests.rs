use super::{config::EdgionGatewayConfig, report_unknown_config_fields};
use std::io::{self, Write};
use std::sync::Arc;
use tracing_subscriber::fmt::MakeWriter;

#[derive(Clone, Default)]
struct TestWriter(Arc<parking_lot::Mutex<Vec<u8>>>);

struct TestWriterGuard<'a>(parking_lot::MutexGuard<'a, Vec<u8>>);

impl Write for TestWriterGuard<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

impl<'a> MakeWriter<'a> for TestWriter {
    type Writer = TestWriterGuard<'a>;

    fn make_writer(&'a self) -> Self::Writer {
        TestWriterGuard(self.0.lock())
    }
}

#[test]
fn unknown_process_config_error_is_structured_and_value_free() {
    let mut file = tempfile::NamedTempFile::new().unwrap();
    file.write_all(b"future_section:\n  nested_secret_value: secret-value-must-not-appear\n")
        .unwrap();
    let loaded = EdgionGatewayConfig::load(EdgionGatewayConfig {
        config_file: Some(file.path().to_string_lossy().into_owned()),
        ..Default::default()
    })
    .unwrap();
    let writer = TestWriter::default();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(writer.clone())
        .finish();

    tracing::subscriber::with_default(subscriber, || {
        report_unknown_config_fields(
            "config/test-gateway.yaml",
            loaded.unknown_fields.paths(),
            loaded.unknown_fields.is_truncated(),
        );
    });

    let output = String::from_utf8(writer.0.lock().clone()).unwrap();
    assert!(output.contains("ERROR"), "{output}");
    assert!(output.contains("future_section"), "{output}");
    assert!(output.contains("config/test-gateway.yaml"), "{output}");
    assert!(output.contains("action=\"ignored\""), "{output}");
    assert!(!output.contains("secret-value-must-not-appear"), "{output}");
    assert!(!output.contains("nested_secret_value"), "{output}");
}

#[test]
fn truncated_unknown_process_config_error_reports_the_cap() {
    let writer = TestWriter::default();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(writer.clone())
        .finish();

    tracing::subscriber::with_default(subscriber, || {
        report_unknown_config_fields("config/test-gateway.yaml", std::iter::empty(), true);
    });

    let output = String::from_utf8(writer.0.lock().clone()).unwrap();
    assert!(output.contains("ERROR"), "{output}");
    assert!(output.contains("reported_fields=32"), "{output}");
    assert!(output.contains("action=\"ignored\""), "{output}");
}
