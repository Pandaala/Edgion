use serde::Serialize;

/// Default capacity for plugin name string.
pub const NAME_CAPACITY: usize = 36;

/// Plugin log entry
/// Fixed structure for plugin execution logging
#[derive(Debug, Clone, Serialize)]
pub struct PluginLog {
    /// Plugin name (pre-allocated with capacity 36)
    pub name: String,
    
    /// Time cost in microseconds (us), None if not measured
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_cost: Option<u64>,
    
    /// Miscellaneous runtime logs, None if not needed
    /// Plugin decides the size when needed
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log: Option<String>,
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
        }
    }

    pub fn add_plugin_log(&mut self, log: &str) {
        self.log
            .get_or_insert_with(|| String::with_capacity(128))
            .push_str(log);
    }
}

/// Stage plugin logs structure
/// Contains logs for a specific execution stage
#[derive(Debug, Clone, Serialize)]
pub struct StagePluginLogs {
    pub stage: &'static str,
    pub logs: Vec<PluginLog>,
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_grouped_logs_serialization() {
        let mut logs: Vec<StagePluginLogs> = Vec::new();
        
        // Add request filters stage
        let mut request_logs = Vec::with_capacity(2);
        let mut log1 = PluginLog::new("cors");
        log1.time_cost = Some(10);
        log1.add_plugin_log("CORS check passed");
        request_logs.push(log1);
        
        let mut log2 = PluginLog::new("csrf");
        log2.time_cost = Some(5);
        request_logs.push(log2);
        
        logs.push(StagePluginLogs {
            stage: "request_filters",
            logs: request_logs,
        });
        
        // Add upstream response filters stage
        let mut upstream_logs = Vec::with_capacity(1);
        let mut log3 = PluginLog::new("ResponseHeaderModifier");
        log3.time_cost = Some(2);
        upstream_logs.push(log3);
        
        logs.push(StagePluginLogs {
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
        let logs: Vec<StagePluginLogs> = Vec::new();
        let json = serde_json::to_string(&logs).unwrap();
        assert_eq!(json, "[]");
    }
    
    #[test]
    fn test_skip_empty_stage() {
        let mut logs: Vec<StagePluginLogs> = Vec::new();
        
        // Manual check: don't push empty stage
        let empty_stage = StagePluginLogs {
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
