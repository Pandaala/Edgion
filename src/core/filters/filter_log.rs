use std::time::Duration;

/// Default capacity for filter name string.
pub const NAME_CAPACITY: usize = 36;

/// Filter log entry
/// Fixed structure for filter execution logging
#[derive(Debug, Clone)]
pub struct FilterLog {
    /// Filter name (pre-allocated with capacity 36)
    pub name: String,
    
    /// Time cost in microseconds (us), None if not measured
    pub timecost: Option<u64>,
    
    /// Miscellaneous runtime logs, None if not needed
    /// Plugin decides the size when needed
    pub misclog: Option<String>,
}

impl FilterLog {
    /// Create a new filter log entry.
    pub fn new(name: &str, timecost: Duration) -> Self {
        let mut n = String::with_capacity(NAME_CAPACITY);
        n.push_str(name);
        
        Self {
            name: n,
            timecost: Some(timecost.as_micros() as u64),
            misclog: None,
        }
    }
    
    /// Create a filter log with name and misclog.
    pub fn with_misclog(name: &str, timecost: Duration, misclog: String) -> Self {
        let mut n = String::with_capacity(NAME_CAPACITY);
        n.push_str(name);
        
        Self {
            name: n,
            timecost: Some(timecost.as_micros() as u64),
            misclog: Some(misclog),
        }
    }
    
    /// Create a filter log with only name.
    pub fn with_name(name: &str) -> Self {
        let mut n = String::with_capacity(NAME_CAPACITY);
        n.push_str(name);
        
        Self {
            name: n,
            timecost: None,
            misclog: None,
        }
    }
    
    /// Set time cost.
    pub fn set_timecost(&mut self, duration: Duration) {
        self.timecost = Some(duration.as_micros() as u64);
    }
    
    /// Set misclog.
    pub fn set_misclog(&mut self, log: String) {
        self.misclog = Some(log);
    }
}
