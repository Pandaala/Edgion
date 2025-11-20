use crate::core::logging::writer::AsyncLogWriter;
use serde_json::json;
use tracing::{Event, Subscriber};
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::Layer;

/// Custom tracing layer for async logging
///
/// Features:
/// - JSON or plain text formatting
/// - Structured field extraction
/// - Async non-blocking writes
/// - Timestamp and metadata inclusion
pub struct AsyncLogLayer {
    pub json_fmt: bool,
    pub writer: AsyncLogWriter,
}

impl<S> Layer<S> for AsyncLogLayer
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
{
    fn on_event(&self, event: &Event<'_>, _ctx: tracing_subscriber::layer::Context<'_, S>) {
        use std::collections::HashMap;
        use tracing::field::{Field, Visit};

        // Visitor to collect all fields
        struct FieldVisitor {
            fields: HashMap<String, String>,
        }

        impl Visit for FieldVisitor {
            fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
                self.fields.insert(field.name().to_string(), format!("{:?}", value));
            }

            fn record_str(&mut self, field: &Field, value: &str) {
                self.fields.insert(field.name().to_string(), value.to_string());
            }

            fn record_i64(&mut self, field: &Field, value: i64) {
                self.fields.insert(field.name().to_string(), value.to_string());
            }

            fn record_u64(&mut self, field: &Field, value: u64) {
                self.fields.insert(field.name().to_string(), value.to_string());
            }

            fn record_bool(&mut self, field: &Field, value: bool) {
                self.fields.insert(field.name().to_string(), value.to_string());
            }
        }

        let mut visitor = FieldVisitor { fields: HashMap::new() };

        // Extract all fields from the event
        event.record(&mut visitor);

        // Get metadata
        let metadata = event.metadata();
        let level = metadata.level();
        let target = metadata.target();
        let timestamp = chrono::Utc::now();

        // Format the log line
        let line = if self.json_fmt {
            // JSON format
            let mut log_json = json!({
                "timestamp": timestamp.to_rfc3339(),
                "level": level.to_string(),
                "target": target,
            });

            // Add all fields to JSON
            if let Some(obj) = log_json.as_object_mut() {
                for (key, value) in visitor.fields {
                    obj.insert(key, json!(value));
                }
            }

            log_json.to_string()
        } else {
            // Plain text format
            let fields_str: Vec<String> = visitor
                .fields
                .iter()
                .map(|(k, v)| {
                    // Special handling for 'message' field
                    if k == "message" {
                        // Strip quotes from message
                        v.trim_matches('"').to_string()
                    } else {
                        format!("{}={}", k, v)
                    }
                })
                .collect();

            format!(
                "{} {:5} [{}] {}",
                timestamp.format("%Y-%m-%d %H:%M:%S%.3f"),
                level,
                target,
                fields_str.join(" ")
            )
        };

        // Send to async writer
        let writer = self.writer.clone();
        tokio::spawn(async move {
            writer.write(line).await;
        });
    }
}
