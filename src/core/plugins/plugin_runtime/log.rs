use serde::Serialize;
use smallvec::SmallVec;

/// Default capacity for plugin name string.
pub const NAME_CAPACITY: usize = 36;

/// Fixed buffer capacity (bytes)
const BUFFER_CAPACITY: usize = 100;

/// Max log entries in fixed buffer
const MAX_LOG_ENTRIES: usize = 20;

/// Helper for serde: skip serializing when false
fn is_false(b: &bool) -> bool {
    !b
}

/// Fixed-size log buffer (栈上，零堆分配)
#[derive(Debug, Clone)]
struct LogBuffer {
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

/// Unlimited log buffer (堆上，无限制)
#[derive(Debug, Clone)]
struct ULogBuffer {
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
    
    /// Fixed-size log buffer (recommended for most plugins)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log: Option<LogBuffer>,
    
    /// Unlimited log buffer (for debug/trace plugins)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ulog: Option<ULogBuffer>,
    
    /// Indicates if fixed buffer was truncated (only serialized when true)
    #[serde(skip_serializing_if = "is_false")]
    pub log_full: bool,
}

impl PluginLog {
    #[inline]
    pub fn new(name: &str) -> Self {
        let mut n = String::with_capacity(NAME_CAPACITY);
        n.push_str(name);
        
        Self {
            name: n,
            time_cost: None,
            log: None,
            ulog: None,
            log_full: false,
        }
    }

    /// Push to fixed buffer (recommended for most plugins)
    #[inline]
    pub fn push(&mut self, log: &str) -> bool {
        let result = self.log.get_or_insert_with(LogBuffer::new).push(log);
        if !result {
            // Fixed buffer is full, mark as truncated
            self.log_full = true;
        }
        result
    }
    
    /// Push to unlimited buffer (for special plugins like debug/trace)
    #[inline]
    pub fn u_push(&mut self, log: &str) {
        self.ulog.get_or_insert_with(ULogBuffer::new).push(log);
    }

    /// Legacy method for backward compatibility (deprecated, use push() instead)
    #[inline]
    #[deprecated(note = "Use push() instead")]
    pub fn add_plugin_log(&mut self, log: &str) {
        self.push(log);
    }
}

/// Stage plugin logs structure
/// Contains logs for a specific execution stage
#[derive(Debug, Clone, Serialize)]
pub struct PluginLogs {
    pub stage: &'static str,
    pub logs: Vec<PluginLog>,
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
        assert!(parsed.get("log_full").is_none());  // false doesn't serialize
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
                assert!(!result);  // Should fail after 20 entries
            }
        }
        
        let json = serde_json::to_string(&log).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        
        // Max 20 entries
        assert!(parsed["log"].as_array().unwrap().len() <= 20);
        // Should be marked as truncated
        assert_eq!(parsed["log_full"].as_bool().unwrap(), true);
    }
    
    #[test]
    fn test_grouped_logs_serialization() {
        let mut logs: Vec<PluginLogs> = Vec::new();
        
        // Add request filters stage
        let mut request_logs = Vec::with_capacity(2);
        let mut log1 = PluginLog::new("cors");
        log1.time_cost = Some(10);
        log1.push("CORS check passed; ");
        request_logs.push(log1);
        
        let mut log2 = PluginLog::new("csrf");
        log2.time_cost = Some(5);
        request_logs.push(log2);
        
        logs.push(PluginLogs {
            stage: "request_filters",
            logs: request_logs,
        });
        
        // Add upstream response filters stage
        let mut upstream_logs = Vec::with_capacity(1);
        let mut log3 = PluginLog::new("ResponseHeaderModifier");
        log3.time_cost = Some(2);
        upstream_logs.push(log3);
        
        logs.push(PluginLogs {
            stage: "upstream_response_filters",
            logs: upstream_logs,
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
        assert_eq!(array[0]["logs"].as_array().unwrap().len(), 2);
        assert_eq!(array[0]["logs"][0]["name"], "cors");
        assert_eq!(array[0]["logs"][0]["time_cost"], 10);
        
        // Check second stage
        assert_eq!(array[1]["stage"], "upstream_response_filters");
        assert_eq!(array[1]["logs"].as_array().unwrap().len(), 1);
    }
    
    #[test]
    fn test_empty_logs_serialization() {
        let logs: Vec<PluginLogs> = Vec::new();
        let json = serde_json::to_string(&logs).unwrap();
        assert_eq!(json, "[]");
    }
    
    #[test]
    fn test_skip_empty_stage() {
        let mut logs: Vec<PluginLogs> = Vec::new();
        
        // Manual check: don't push empty stage
        let empty_stage = PluginLogs {
            stage: "request_filters",
            logs: Vec::new(),
        };
        
        if !empty_stage.logs.is_empty() {
            logs.push(empty_stage);
        }
        
        assert!(logs.is_empty());
        
        let json = serde_json::to_string(&logs).unwrap();
        assert_eq!(json, "[]");
    }
}
