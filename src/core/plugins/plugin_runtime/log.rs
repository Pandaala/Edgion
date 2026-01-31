use serde::Serialize;
use smallvec::SmallVec;

// LocalObjectReference no longer needed - refer_to is now just a String

/// Default capacity for plugin name string.
pub const NAME_CAPACITY: usize = 36;

/// Fixed buffer capacity (bytes)
const BUFFER_CAPACITY: usize = 100;

/// Max log entries in fixed buffer
const MAX_LOG_ENTRIES: usize = 20;

/// Fixed-size log buffer (stack-allocated, zero heap allocation)
#[derive(Debug, Clone)]
pub struct LogBuffer {
    buffer: SmallVec<[u8; BUFFER_CAPACITY]>,
    positions: SmallVec<[usize; MAX_LOG_ENTRIES]>,
}

impl LogBuffer {
    fn new() -> Self {
        Self {
            buffer: SmallVec::new(),
            positions: SmallVec::new(),
        }
    }

    #[inline]
    fn push(&mut self, log: &str) -> bool {
        // Check capacity limits
        if self.positions.len() >= MAX_LOG_ENTRIES {
            return false;
        }
        if self.buffer.len() + log.len() > BUFFER_CAPACITY {
            return false;
        }

        // Write to buffer
        self.buffer.extend_from_slice(log.as_bytes());
        self.positions.push(self.buffer.len());
        true
    }
}

impl Serialize for LogBuffer {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeSeq;
        let mut seq = serializer.serialize_seq(Some(self.positions.len()))?;
        let mut start = 0;
        for &end in &self.positions {
            let slice = &self.buffer[start..end];
            let s = std::str::from_utf8(slice).map_err(serde::ser::Error::custom)?;
            seq.serialize_element(s)?;
            start = end;
        }
        seq.end()
    }
}

/// Unlimited log buffer (heap-allocated, unlimited)
#[derive(Debug, Clone)]
pub struct ULogBuffer {
    buffer: String,
    positions: Vec<usize>,
}

impl ULogBuffer {
    fn new() -> Self {
        Self {
            buffer: String::with_capacity(256),
            positions: Vec::with_capacity(32),
        }
    }

    #[inline]
    fn push(&mut self, log: &str) {
        self.buffer.push_str(log);
        self.positions.push(self.buffer.len());
    }
}

impl Serialize for ULogBuffer {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeSeq;
        let mut seq = serializer.serialize_seq(Some(self.positions.len()))?;
        let mut start = 0;
        for &end in &self.positions {
            seq.serialize_element(&self.buffer[start..end])?;
            start = end;
        }
        seq.end()
    }
}

/// Plugin log entry
/// Fixed structure for plugin execution logging
#[derive(Debug, Clone, Serialize)]
pub struct PluginLog {
    /// Plugin name (pre-allocated with capacity 36)
    pub name: String,

    /// Time cost in microseconds (us), None if not measured
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_cost: Option<u64>,

    /// Condition skip reason, None if not skipped
    /// e.g., "skip:keyExist,hdr:X-Internal"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cond_skip: Option<String>,

    /// Fixed-size log buffer (recommended for most plugins)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log: Option<LogBuffer>,

    /// Unlimited log buffer (for debug/trace plugins)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ulog: Option<ULogBuffer>,

    /// Indicates if fixed buffer was truncated (only serialized when true)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log_full: Option<bool>,

    /// ExtensionRef reference name (for ExtensionRef filter only)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refer_to: Option<String>,
}

impl PluginLog {
    #[inline]
    pub fn new(name: &str) -> Self {
        let mut n = String::with_capacity(NAME_CAPACITY);
        n.push_str(name);

        Self {
            name: n,
            time_cost: None,
            cond_skip: None,
            log: None,
            ulog: None,
            log_full: None,
            refer_to: None,
        }
    }

    /// Push to fixed buffer (recommended for most plugins)
    #[inline]
    pub fn push(&mut self, log: &str) -> bool {
        let result = self.log.get_or_insert_with(LogBuffer::new).push(log);
        if !result {
            // Fixed buffer is full, mark as truncated
            self.log_full = Some(true);
        }
        result
    }

    /// Push to unlimited buffer (for special plugins like debug/trace)
    #[inline]
    pub fn u_push(&mut self, log: &str) {
        self.ulog.get_or_insert_with(ULogBuffer::new).push(log);
    }

    /// Helper method for tests: checks if log buffer contains a substring
    #[cfg(test)]
    pub fn contains(&self, substr: &str) -> bool {
        if let Some(ref log_buf) = self.log {
            // Check in fixed-size buffer
            if let Ok(content) = std::str::from_utf8(&log_buf.buffer) {
                if content.contains(substr) {
                    return true;
                }
            }
        }
        if let Some(ref ulog_buf) = self.ulog {
            // Check in unlimited buffer
            if ulog_buf.buffer.contains(substr) {
                return true;
            }
        }
        false
    }

    /// Set condition skip reason
    /// Format: "action:type,detail" e.g., "skip:keyExist,hdr:X-Internal"
    #[inline]
    pub fn set_cond_skip(&mut self, reason: String) {
        self.cond_skip = Some(reason);
    }

    /// Check if plugin was skipped by condition
    #[inline]
    pub fn is_cond_skipped(&self) -> bool {
        self.cond_skip.is_some()
    }

    /// Set ExtensionRef reference name (for ExtensionRef filter only)
    #[inline]
    pub fn set_refer_to(&mut self, name: String) {
        self.refer_to = Some(name);
    }

    /// Legacy method for backward compatibility (deprecated, use push() instead)
    #[inline]
    #[deprecated(note = "Use push() instead")]
    pub fn add_plugin_log(&mut self, log: &str) {
        self.push(log);
    }
}

/// EdgionPlugins execution log (flattened in edgion_plugins array)
#[derive(Debug, Clone, Serialize)]
pub struct EdgionPluginsLog {
    /// EdgionPlugins resource name
    pub name: String,
    /// Plugin logs within this EdgionPlugins
    pub logs: Vec<PluginLog>,
}

/// Token for safely pushing logs to a specific EdgionPluginsLog.
///
/// This token is returned by `start_edgion_plugins_log` and must be used
/// with `push_to_edgion_plugins_log` to append plugin logs. The token
/// includes depth validation to prevent misuse across nested scopes.
///
/// The token is intentionally NOT Clone or Copy to prevent accidental
/// sharing across different scopes.
#[derive(Debug)]
pub struct EdgionPluginsLogToken {
    /// Index in pending_edgion_plugins_logs
    pub(crate) idx: usize,
    /// Depth at creation time (for validation)
    pub(crate) depth: usize,
}

impl EdgionPluginsLogToken {
    /// Create a new token (internal use only)
    pub(crate) fn new(idx: usize, depth: usize) -> Self {
        Self { idx, depth }
    }

    /// Get the index
    #[inline]
    pub fn idx(&self) -> usize {
        self.idx
    }

    /// Get the depth at creation time
    #[inline]
    pub fn depth(&self) -> usize {
        self.depth
    }
}

/// Stage logs structure (renamed from PluginLogs)
/// Contains logs for a specific execution stage
#[derive(Debug, Clone, Serialize)]
pub struct StageLogs {
    /// Stage name (e.g., "request_filters", "upstream_response_filters")
    pub stage: &'static str,
    /// Filter logs from HTTPRoute/GRPCRoute filters
    pub filters: Vec<PluginLog>,
    /// EdgionPlugins logs (flattened, in execution order)
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub edgion_plugins: Vec<EdgionPluginsLog>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fixed_buffer() {
        let mut log = PluginLog::new("test");
        assert!(log.push("aaa; "));
        assert!(log.push("bbb; "));

        let json = serde_json::to_string(&log).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed["log"].as_array().unwrap().len(), 2);
        assert_eq!(parsed["log"][0], "aaa; ");
        assert_eq!(parsed["log"][1], "bbb; ");
        assert!(parsed.get("ulog").is_none());
        assert!(parsed.get("log_full").is_none()); // false doesn't serialize
    }

    #[test]
    fn test_unlimited_buffer() {
        let mut log = PluginLog::new("debug");

        for i in 0..100 {
            log.u_push(&format!("entry {}; ", i));
        }

        let json = serde_json::to_string(&log).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed["ulog"].as_array().unwrap().len(), 100);
        assert!(parsed.get("log").is_none());
    }

    #[test]
    fn test_both_buffers() {
        let mut log = PluginLog::new("mixed");
        log.push("quick; ");
        log.u_push("detailed info; ");

        let json = serde_json::to_string(&log).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed["log"].as_array().unwrap().len(), 1);
        assert_eq!(parsed["ulog"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn test_fixed_capacity_limit() {
        let mut log = PluginLog::new("test");

        // Fill buffer
        for i in 0..30 {
            let result = log.push(&format!("entry {}; ", i));
            if i >= 20 {
                assert!(!result); // Should fail after 20 entries
            }
        }

        let json = serde_json::to_string(&log).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        // Max 20 entries
        assert!(parsed["log"].as_array().unwrap().len() <= 20);
        // Should be marked as truncated
        assert!(parsed["log_full"].as_bool().unwrap());
    }

    #[test]
    fn test_grouped_logs_serialization() {
        let mut logs: Vec<StageLogs> = Vec::new();

        // Add request filters stage
        let mut request_filters = Vec::with_capacity(2);
        let mut log1 = PluginLog::new("cors");
        log1.time_cost = Some(10);
        log1.push("CORS check passed; ");
        request_filters.push(log1);

        let mut log2 = PluginLog::new("csrf");
        log2.time_cost = Some(5);
        request_filters.push(log2);

        logs.push(StageLogs {
            stage: "request_filters",
            filters: request_filters,
            edgion_plugins: Vec::new(),
        });

        // Add upstream response filters stage
        let mut upstream_filters = Vec::with_capacity(1);
        let mut log3 = PluginLog::new("ResponseHeaderModifier");
        log3.time_cost = Some(2);
        upstream_filters.push(log3);

        logs.push(StageLogs {
            stage: "upstream_response_filters",
            filters: upstream_filters,
            edgion_plugins: Vec::new(),
        });

        // Serialize
        let json = serde_json::to_string(&logs).unwrap();

        // Verify structure
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(parsed.is_array());

        let array = parsed.as_array().unwrap();
        assert_eq!(array.len(), 2);

        // Check first stage
        assert_eq!(array[0]["stage"], "request_filters");
        assert_eq!(array[0]["filters"].as_array().unwrap().len(), 2);
        assert_eq!(array[0]["filters"][0]["name"], "cors");
        assert_eq!(array[0]["filters"][0]["time_cost"], 10);

        // Check second stage
        assert_eq!(array[1]["stage"], "upstream_response_filters");
        assert_eq!(array[1]["filters"].as_array().unwrap().len(), 1);
        // edgion_plugins should not appear when empty
        assert!(array[0].get("edgion_plugins").is_none());
    }

    #[test]
    fn test_empty_logs_serialization() {
        let logs: Vec<StageLogs> = Vec::new();
        let json = serde_json::to_string(&logs).unwrap();
        assert_eq!(json, "[]");
    }

    #[test]
    fn test_skip_empty_stage() {
        let mut logs: Vec<StageLogs> = Vec::new();

        // Manual check: don't push empty stage
        let empty_stage = StageLogs {
            stage: "request_filters",
            filters: Vec::new(),
            edgion_plugins: Vec::new(),
        };

        if !empty_stage.filters.is_empty() {
            logs.push(empty_stage);
        }

        assert!(logs.is_empty());

        let json = serde_json::to_string(&logs).unwrap();
        assert_eq!(json, "[]");
    }

    #[test]
    fn test_stage_logs_with_edgion_plugins() {
        let stage = StageLogs {
            stage: "request_filters",
            filters: vec![{
                let mut log = PluginLog::new("ExtensionRef");
                // refer_to is now simplified to just the name string
                log.set_refer_to("auth-plugins".to_string());
                log
            }],
            edgion_plugins: vec![EdgionPluginsLog {
                name: "auth-plugins".to_string(),
                logs: vec![{
                    let mut log = PluginLog::new("BasicAuth");
                    log.time_cost = Some(45);
                    log.push("User authenticated");
                    log
                }],
            }],
        };

        let json = serde_json::to_string(&stage).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        // Check filters - refer_to is now just a string
        assert_eq!(parsed["filters"][0]["name"], "ExtensionRef");
        assert_eq!(parsed["filters"][0]["refer_to"], "auth-plugins");
        // ExtensionRef should not have time_cost
        assert!(parsed["filters"][0].get("time_cost").is_none());

        // Check edgion_plugins
        assert_eq!(parsed["edgion_plugins"][0]["name"], "auth-plugins");
        assert_eq!(parsed["edgion_plugins"][0]["logs"][0]["name"], "BasicAuth");
        assert_eq!(parsed["edgion_plugins"][0]["logs"][0]["time_cost"], 45);
    }
}
