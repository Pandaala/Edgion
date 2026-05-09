use crate::core::gateway::observe::logs::{LogBuffer, ULogBuffer};
use serde::Serialize;
use smallvec::SmallVec;

// LocalObjectReference no longer needed - refer_to is now just a String

/// Default capacity for plugin name string.
pub const NAME_CAPACITY: usize = 36;

/// A single argument to [`PluginLog::push_fmt`].
///
/// - `Plain` — written as-is regardless of sensitive mode.
/// - `Sensitive` — passed through [`crate::types::sensitive::apply_sensitive`]; masked or wrapped
///   when the active mode is Replace or Wrap.
///
/// Construct via `From<&str>` (plain) or [`sensitive()`] (sensitive).
/// Use the [`la!`] macro to build a `&[LogArg]` slice without explicit `.into()` calls.
#[derive(Debug)]
pub enum LogArg<'a> {
    Plain(&'a str),
    Sensitive(&'a str),
}

impl<'a> From<&'a str> for LogArg<'a> {
    #[inline]
    fn from(s: &'a str) -> Self {
        LogArg::Plain(s)
    }
}

/// Mark an argument as sensitive so it is masked when the active mode is Replace or Wrap.
///
/// Plain arguments need no annotation — only wrap sensitive ones.
///
/// ```ignore
/// log.push_fmt("user {} from {}", la!["admin", sensitive(&client_ip)]);
/// ```
#[inline]
pub fn sensitive(s: &str) -> LogArg<'_> {
    LogArg::Sensitive(s)
}

/// Build a `&[LogArg]` slice for [`PluginLog::push_fmt`] / [`PluginLog::u_push_fmt`].
///
/// Plain `&str` values need no wrapper; only sensitive values use [`sensitive()`].
///
/// ```ignore
/// log.push_fmt("key {} set by {}", la![key, sensitive(&user_input)]);
/// ```
#[macro_export]
macro_rules! la {
    () => { &[] as &[$crate::core::gateway::plugins::runtime::log::LogArg] };
    ($($e:expr),+ $(,)?) => {
        &[$( $crate::core::gateway::plugins::runtime::log::LogArg::from($e) ),+]
    };
}
pub use la;

/// Render `fmt` with `{}` placeholders substituted by `args` directly into `buf`.
/// Sensitive args run through the active masking mode.
#[inline]
fn format_into<A: smallvec::Array<Item = u8>>(buf: &mut SmallVec<A>, fmt: &str, args: &[LogArg<'_>]) {
    let mut arg_iter = args.iter();
    let mut rest = fmt;
    while let Some(pos) = rest.find("{}") {
        buf.extend_from_slice(rest[..pos].as_bytes());
        rest = &rest[pos + 2..];
        if let Some(arg) = arg_iter.next() {
            match arg {
                LogArg::Plain(s) => buf.extend_from_slice(s.as_bytes()),
                LogArg::Sensitive(s) => {
                    crate::types::sensitive::apply_sensitive(s, |w| buf.extend_from_slice(w.as_bytes()));
                }
            }
        }
    }
    buf.extend_from_slice(rest.as_bytes());
}

/// Plugin log entry
/// Fixed structure for plugin execution logging
#[derive(Debug, Clone, Serialize)]
pub struct PluginLog {
    /// Plugin name (pre-allocated with capacity 36)
    pub name: String,

    /// Operator-defined label propagated from the EdgionPlugins entry's
    /// `alias` field. Independent of `name`. Stored separately from the
    /// 100-byte LogBuffer so it never reduces buffer space available to
    /// `push` / `push_err`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,

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
            alias: None,
            time_cost: None,
            cond_skip: None,
            log: None,
            ulog: None,
            log_full: None,
            refer_to: None,
        }
    }

    /// Construct a PluginLog carrying both the type-name and an optional
    /// operator-defined alias. `alias = None` is equivalent to `new(name)`.
    #[inline]
    pub fn with_alias(name: &str, alias: Option<&str>) -> Self {
        let mut log = Self::new(name);
        log.alias = alias.map(|s| s.to_owned());
        log
    }

    /// Push to fixed buffer (recommended for non-error paths).
    ///
    /// For error entries, use [`PluginLog::push_err`] — it enforces a
    /// `&'static str` argument and unified `"err: "` prefix, blocking
    /// accidental token/claim leakage via `format!` or error `Display`.
    #[inline]
    pub fn push(&mut self, log: &str) -> bool {
        let result = self.log.get_or_insert_with(LogBuffer::new).push(log);
        if !result {
            // Fixed buffer is full, mark as truncated
            self.log_full = Some(true);
        }
        result
    }

    /// Push an error entry with unified `"err: "` prefix.
    ///
    /// `kind` is `&'static str` so the compiler rejects `format!` / `err.to_string()`
    /// at the call site — the only way to include dynamic detail is via `ctx.err_log`
    /// (access log) after explicit redaction. This is the required API for all new
    /// plugin error logging; `push(&format!("...: {}", err))` is forbidden for error
    /// paths because error `Display` may carry tokens, claims, or raw user input.
    ///
    /// Serialized entry: `"err: <kind>; "`. Plugin identity comes from
    /// [`PluginLog::name`], so `kind` should be the event only (e.g.
    /// `"token refresh fail"`), not `"<plugin>:<event>"`.
    ///
    /// Format spec + examples: `skills/03-coding/observability/00-access-log.md`
    /// (section "Error entry API constraints"). Safety rationale: `skills/03-coding/01-log-safety.md`.
    #[inline]
    pub fn push_err(&mut self, kind: &'static str) -> bool {
        let result = self.log.get_or_insert_with(LogBuffer::new).push_err(kind);
        if !result {
            self.log_full = Some(true);
        }
        result
    }

    /// Push to unbounded buffer (for debug/trace plugins). Returns false if buffer limit reached.
    ///
    /// For error entries, use [`PluginLog::u_push_err`] — see [`PluginLog::push_err`] for rationale.
    #[inline]
    pub fn u_push(&mut self, log: &str) -> bool {
        self.ulog.get_or_insert_with(ULogBuffer::new).push(log)
    }

    /// Unbounded-buffer counterpart of [`PluginLog::push_err`].
    #[inline]
    pub fn u_push_err(&mut self, kind: &'static str) -> bool {
        self.ulog.get_or_insert_with(ULogBuffer::new).push_err(kind)
    }

    /// Push a user-controlled dynamic value through the active masking mode.
    ///
    /// Truncates to the last UTF-8 char boundary at or before 64 bytes to respect
    /// LogBuffer's 100-byte limit after any wrapper overhead.
    /// Use this for DSL/plugin content that may contain credentials or PII.
    /// For static event labels, use [`push_err`] instead.
    #[inline]
    pub fn push_sensitive(&mut self, value: &str) -> bool {
        // Reserve bytes for wrap prefix+suffix so the final entry fits LogBuffer's 100-byte limit.
        // Default wrap overhead is 7 bytes ("<E>" + "</E>"); operators can configure longer strings.
        let overhead = crate::types::sensitive::wrap_overhead();
        let max_value_bytes = (100usize.saturating_sub(overhead)).min(64);
        let end = (0..=value.len().min(max_value_bytes))
            .rev()
            .find(|&i| value.is_char_boundary(i))
            .unwrap_or(0);
        let truncated = &value[..end];
        crate::types::sensitive::apply_sensitive(truncated, |w| {
            let result = self.log.get_or_insert_with(LogBuffer::new).push(w);
            if !result {
                self.log_full = Some(true);
            }
            result
        })
    }

    /// Unbounded-buffer counterpart of [`push_sensitive`]. No truncation —
    /// ULogBuffer handles its own 512-byte limit.
    #[inline]
    pub fn u_push_sensitive(&mut self, value: &str) -> bool {
        crate::types::sensitive::apply_sensitive(value, |w| self.ulog.get_or_insert_with(ULogBuffer::new).push(w))
    }

    /// Push an error entry whose value is dynamic — atomically writes
    /// `"err: <masked-value>; "` as a single buffer entry.
    ///
    /// The dynamic counterpart of [`push_err`] for callers that cannot supply a
    /// `&'static str` (notably the DSL `log_error()` builtin). Output format
    /// matches [`push_err`] so downstream parsers grepping for `err:` see both.
    /// The value is masked through the active sensitive mode and truncated to
    /// fit inside the 100-byte LogBuffer entry after accounting for prefix,
    /// suffix, and any wrap overhead.
    pub fn push_err_sensitive(&mut self, value: &str) -> bool {
        const PREFIX: &str = "err: ";
        const SUFFIX: &str = "; ";
        let overhead = crate::types::sensitive::wrap_overhead();
        let max_value_bytes = (100usize.saturating_sub(PREFIX.len() + SUFFIX.len() + overhead)).min(64);
        let end = (0..=value.len().min(max_value_bytes))
            .rev()
            .find(|&i| value.is_char_boundary(i))
            .unwrap_or(0);
        let truncated = &value[..end];
        let mut entry = String::with_capacity(PREFIX.len() + SUFFIX.len() + max_value_bytes + overhead);
        entry.push_str(PREFIX);
        crate::types::sensitive::apply_sensitive(truncated, |w| entry.push_str(w));
        entry.push_str(SUFFIX);
        let result = self.log.get_or_insert_with(LogBuffer::new).push(&entry);
        if !result {
            self.log_full = Some(true);
        }
        result
    }

    /// Push a format-string log entry where only explicitly marked args are masked.
    ///
    /// The template string is never masked. Each `{}` placeholder is replaced by the
    /// corresponding [`LogArg`]: `Plain` args are inserted verbatim; `Sensitive` args are
    /// passed through the active masking mode (replace/wrap). Use the [`la!`] macro to
    /// build the args slice conveniently.
    ///
    /// ```ignore
    /// log.push_fmt("key {} set", la![rule.name.as_str()]);
    /// log.push_fmt("user {} from {}", la!["admin", sensitive(&client_ip)]);
    /// ```
    pub fn push_fmt(&mut self, fmt: &str, args: &[LogArg<'_>]) -> bool {
        // Stack-inlined scratch buffer: LogBuffer caps each entry at 100 bytes,
        // so 128 bytes inline always avoids heap allocation here.
        let mut buf: SmallVec<[u8; 128]> = SmallVec::new();
        format_into(&mut buf, fmt, args);
        let s = std::str::from_utf8(&buf).unwrap_or("");
        let ok = self.log.get_or_insert_with(LogBuffer::new).push(s);
        if !ok {
            self.log_full = Some(true);
        }
        ok
    }

    /// Unbounded-buffer counterpart of [`push_fmt`].
    pub fn u_push_fmt(&mut self, fmt: &str, args: &[LogArg<'_>]) -> bool {
        // 256 bytes inline covers typical entries without heap; oversize entries
        // spill once via SmallVec growth, still better than always-heap String.
        let mut buf: SmallVec<[u8; 256]> = SmallVec::new();
        format_into(&mut buf, fmt, args);
        let s = std::str::from_utf8(&buf).unwrap_or("");
        self.ulog.get_or_insert_with(ULogBuffer::new).push(s)
    }

    /// Helper method for tests: checks if log buffer contains a substring
    #[cfg(test)]
    pub fn contains(&self, substr: &str) -> bool {
        if let Some(ref log_buf) = self.log {
            if log_buf.contains(substr) {
                return true;
            }
        }
        if let Some(ref ulog_buf) = self.ulog {
            if ulog_buf.contains(substr) {
                return true;
            };
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
/// with `push_to_edgion_plugins_log` to append plugin logs. It carries the
/// index of the active edgion-plugins log entry.
///
/// The token is intentionally NOT Clone or Copy to prevent accidental
/// sharing across different scopes.
#[derive(Debug)]
pub struct EdgionPluginsLogToken {
    /// Index in pending_edgion_plugins_logs
    pub(crate) idx: usize,
}

impl EdgionPluginsLogToken {
    /// Create a new token (internal use only)
    pub(crate) fn new(idx: usize) -> Self {
        Self { idx }
    }

    /// Get the index
    #[inline]
    pub fn idx(&self) -> usize {
        self.idx
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
    use crate::types::sensitive::SENSITIVE_MODE;
    use std::sync::atomic::Ordering;
    use std::sync::Mutex;

    static MODE_LOCK: Mutex<()> = Mutex::new(());

    struct ModeGuard<'a> {
        prev: u8,
        // Held purely for RAII: serializes tests that mutate SENSITIVE_MODE.
        _lock: std::sync::MutexGuard<'a, ()>,
    }
    impl Drop for ModeGuard<'_> {
        fn drop(&mut self) {
            SENSITIVE_MODE.store(self.prev, Ordering::SeqCst);
        }
    }
    fn set_mode(mode: u8) -> ModeGuard<'static> {
        let guard = MODE_LOCK.lock().unwrap();
        let prev = SENSITIVE_MODE.swap(mode, Ordering::SeqCst);
        ModeGuard { prev, _lock: guard }
    }

    #[test]
    fn test_push_fmt_plain_args_never_masked() {
        let _g = set_mode(1); // Replace mode
        let mut log = PluginLog::new("test");
        log.push_fmt("key {} set", la!["my-key"]);
        assert!(log.contains("key my-key set"));
    }

    #[test]
    fn test_push_fmt_sensitive_arg_replaced() {
        let _g = set_mode(1); // Replace mode
        let mut log = PluginLog::new("test");
        log.push_fmt("user {} from {}", la!["admin", sensitive("1.2.3.4")]);
        assert!(log.contains("user admin from ***"));
        assert!(!log.contains("1.2.3.4"));
    }

    #[test]
    fn test_push_fmt_sensitive_arg_wrapped() {
        let _g = set_mode(2); // Wrap mode
        let mut log = PluginLog::new("test");
        log.push_fmt("user {} from {}", la!["admin", sensitive("1.2.3.4")]);
        assert!(log.contains("user admin from <E>1.2.3.4</E>"));
    }

    #[test]
    fn test_push_fmt_off_mode_writes_plain() {
        let _g = set_mode(0); // Off mode
        let mut log = PluginLog::new("test");
        log.push_fmt("user {} from {}", la!["admin", sensitive("1.2.3.4")]);
        assert!(log.contains("user admin from 1.2.3.4"));
    }

    #[test]
    fn test_push_fmt_no_placeholders() {
        let _g = set_mode(1);
        let mut log = PluginLog::new("test");
        log.push_fmt("static message; ", la![]);
        assert!(log.contains("static message; "));
    }

    #[test]
    fn test_push_sensitive_off_writes_plain() {
        let _g = set_mode(0);
        let mut log = PluginLog::new("test");
        log.push_sensitive("hello world");
        assert!(log.contains("hello world"));
    }

    #[test]
    fn test_push_sensitive_replace_writes_stars() {
        let _g = set_mode(1);
        let mut log = PluginLog::new("test");
        log.push_sensitive("secret-token-value");
        assert!(log.contains("***"));
        assert!(!log.contains("secret-token-value"));
    }

    #[test]
    fn test_push_sensitive_wrap_wraps_value() {
        let _g = set_mode(2);
        let mut log = PluginLog::new("test");
        log.push_sensitive("user@example.com");
        assert!(log.contains("<E>user@example.com</E>"));
    }

    #[test]
    fn test_push_sensitive_truncates_at_utf8_boundary() {
        let _g = set_mode(0);
        // 63 ASCII bytes + 2-byte 'é' (bytes 63-64) + "trail"
        let mut s = "a".repeat(63);
        s.push('é');
        s.push_str("trail");
        assert!(s.len() > 64);
        let mut log = PluginLog::new("test");
        log.push_sensitive(&s);
        assert!(!log.contains("trail"));
    }

    #[test]
    fn test_u_push_sensitive_wrap() {
        let _g = set_mode(2);
        let mut log = PluginLog::new("test");
        log.u_push_sensitive("dynamic-debug-value");
        assert!(log.ulog.as_ref().unwrap().contains("<E>dynamic-debug-value</E>"));
    }

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
    fn test_push_err_prefix_and_suffix() {
        let mut log = PluginLog::new("openid_connect");
        assert!(log.push_err("token refresh fail"));
        assert!(log.push_err("jwt expired"));

        let json = serde_json::to_string(&log).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed["name"], "openid_connect");
        let entries = parsed["log"].as_array().unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0], "err: token refresh fail; ");
        assert_eq!(entries[1], "err: jwt expired; ");
    }

    #[test]
    fn test_push_err_capacity_limit_marks_truncated() {
        let mut log = PluginLog::new("t");
        // 100B buffer; each "err: xx; " = ~10B. Push until rejection.
        let mut ok_count = 0;
        for _ in 0..30 {
            if log.push_err("xx") {
                ok_count += 1;
            }
        }
        assert!(ok_count < 30);
        let json = serde_json::to_string(&log).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(parsed["log_full"].as_bool().unwrap());
    }

    #[test]
    fn test_u_push_err_format() {
        let mut log = PluginLog::new("dbg");
        assert!(log.u_push_err("discovery fail"));
        let json = serde_json::to_string(&log).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["ulog"][0], "err: discovery fail; ");
    }

    #[test]
    fn test_mixed_push_and_push_err_order_preserved() {
        let mut log = PluginLog::new("mixed");
        assert!(log.push("OK u=alice; "));
        assert!(log.push_err("token refresh fail"));
        assert!(log.push("retry scheduled; "));

        let json = serde_json::to_string(&log).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        let entries = parsed["log"].as_array().unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0], "OK u=alice; ");
        assert_eq!(entries[1], "err: token refresh fail; ");
        assert_eq!(entries[2], "retry scheduled; ");
    }

    #[test]
    fn test_push_err_exact_byte_cap_boundary() {
        // 100B buffer, "err: xx; " = 9B each. 11 entries = 99B fits,
        // 12th (+9B => 108B) must fail and mark log_full.
        let mut log = PluginLog::new("b");
        for i in 0..11 {
            assert!(log.push_err("xx"), "entry {} should fit", i);
        }
        assert!(!log.push_err("xx"));
        assert_eq!(log.log_full, Some(true));
        // Further pushes (err or plain) also rejected.
        assert!(!log.push_err("x"));
        assert!(!log.push("y; "));
    }

    #[test]
    fn test_unlimited_buffer() {
        let mut log = PluginLog::new("debug");
        let mut accepted = 0usize;

        for i in 0..100 {
            if log.u_push(&format!("entry {}; ", i)) {
                accepted += 1;
            }
        }

        let json = serde_json::to_string(&log).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        let entries = parsed["ulog"].as_array().unwrap();
        assert_eq!(entries.len(), accepted);
        assert!(accepted > 0);
        assert!(accepted < 100, "ULogBuffer now has a byte limit and should truncate");
        assert!(parsed.get("log").is_none());
        assert!(!log.u_push(&"x".repeat(1024)));
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
    fn plugin_log_alias_field_serializes_only_when_set() {
        // alias unset -> field omitted
        let log_no_alias = PluginLog::new("KeyAuth");
        let json = serde_json::to_string(&log_no_alias).unwrap();
        assert!(!json.contains("alias"), "alias must be omitted when None: {json}");

        // alias set via with_alias -> field present and verbatim
        let log_with_alias = PluginLog::with_alias("KeyAuth", Some("auth-internal"));
        let json = serde_json::to_string(&log_with_alias).unwrap();
        assert!(
            json.contains(r#""alias":"auth-internal""#),
            "alias must serialize verbatim: {json}"
        );
        assert_eq!(log_with_alias.name, "KeyAuth");

        // with_alias(name, None) is equivalent to new(name)
        let log_none = PluginLog::with_alias("KeyAuth", None);
        let json = serde_json::to_string(&log_none).unwrap();
        assert!(
            !json.contains("alias"),
            "alias must be omitted when with_alias passed None: {json}"
        );
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
