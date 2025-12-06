/// Default capacity for plugin name string.
pub const NAME_CAPACITY: usize = 36;

/// Plugin log entry
/// Fixed structure for plugin execution logging
#[derive(Debug, Clone)]
pub struct PluginLog {
    /// Plugin name (pre-allocated with capacity 36)
    pub name: String,
    
    /// Time cost in microseconds (us), None if not measured
    pub time_cost: Option<u64>,
    
    /// Miscellaneous runtime logs, None if not needed
    /// Plugin decides the size when needed
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
