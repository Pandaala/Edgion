use crate::observe::logs::buffer::ULOG_MAX_BUFFER;
use crate::observe::logs::{LogBuffer, ULogBuffer};
use serde::Serialize;
use smallvec::SmallVec;
use std::io::{Cursor, Write};

/// Default capacity for plugin name string.
pub const NAME_CAPACITY: usize = 36;

/// A single argument to [`PluginLog::push_fmt`].
///
/// - `Plain` — written as-is regardless of sensitive mode.
/// - `Unsigned` — a non-sensitive diagnostic integer, formatted without a String.
/// - `Sensitive` — passed through [`edgion_resources::sensitive::apply_sensitive`]; masked or wrapped
///   when the active mode is Replace or Wrap.
///
/// Construct via `From<&str>`, `From<u16>` or `From<u64>` for non-sensitive
/// diagnostics, or [`sensitive()`] for sensitive text.
/// Use the [`la!`] macro to build a `&[LogArg]` slice without explicit `.into()` calls.
#[derive(Debug)]
pub enum LogArg<'a> {
    Plain(&'a str),
    Unsigned(u64),
    Sensitive(&'a str),
}

impl From<u64> for LogArg<'_> {
    fn from(value: u64) -> Self {
        Self::Unsigned(value)
    }
}

impl From<u16> for LogArg<'_> {
    fn from(value: u16) -> Self {
        Self::Unsigned(u64::from(value))
    }
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
/// log.push_fmt("user {} from {}", la!["admin", sensitive(&remote_ip)]);
/// ```
#[inline]
pub fn sensitive(s: &str) -> LogArg<'_> {
    LogArg::Sensitive(s)
}

/// Build a `&[LogArg]` slice for [`PluginLog::push_fmt`].
///
/// Plain `&str`, `u16` and `u64` values need no wrapper; sensitive text uses [`sensitive()`].
///
/// ```ignore
/// log.push_fmt("key {} set by {}", la![key, sensitive(&user_input)]);
/// ```
#[macro_export]
macro_rules! la {
    () => { &[] as &[$crate::plugins::runtime::log::LogArg] };
    ($($e:expr),+ $(,)?) => {
        &[$( $crate::plugins::runtime::log::LogArg::from($e) ),+]
    };
}
pub use la;

/// Render `fmt` with `{}` placeholders substituted by `args` directly into `buf`.
/// Sensitive args run through the active masking mode.
#[inline]
fn format_into<A: smallvec::Array<Item = u8>>(buf: &mut SmallVec<A>, fmt: &str, args: &[LogArg<'_>]) -> bool {
    fn append<A: smallvec::Array<Item = u8>>(buf: &mut SmallVec<A>, value: &str) -> bool {
        if value.len() > ULOG_MAX_BUFFER - buf.len() {
            return false;
        }
        buf.extend_from_slice(value.as_bytes());
        true
    }

    let mut arg_iter = args.iter();
    let mut rest = fmt;
    while let Some(pos) = rest.find("{}") {
        if !append(buf, &rest[..pos]) {
            return false;
        }
        rest = &rest[pos + 2..];
        if let Some(arg) = arg_iter.next() {
            let accepted = match arg {
                LogArg::Plain(s) => append(buf, s),
                LogArg::Unsigned(value) => {
                    // Every decimal u64 fits in 20 ASCII bytes. Only the
                    // primitive integer formatter runs here, never user Display.
                    let mut number = Cursor::new([0u8; 20]);
                    if write!(&mut number, "{value}").is_err() {
                        return false;
                    }
                    let len = number.position() as usize;
                    match std::str::from_utf8(&number.get_ref()[..len]) {
                        Ok(text) => append(buf, text),
                        Err(_) => false,
                    }
                }
                LogArg::Sensitive(s) => {
                    edgion_resources::sensitive::apply_sensitive_bounded(s, ULOG_MAX_BUFFER - buf.len(), |w| {
                        buf.extend_from_slice(w.as_bytes());
                    })
                    .is_some()
                }
            };
            if !accepted {
                return false;
            }
        }
    }
    append(buf, rest)
}

/// Plugin log entry — fixed structure for plugin execution logging.
///
/// Serialization is custom (see `impl Serialize` below): the two physical
/// buffers `log` and `ulog` are flattened into a single `"log"` JSON array,
/// and every `Option` field is omitted when `None`.
#[derive(Debug, Clone)]
pub struct PluginLog {
    /// Plugin name (pre-allocated with capacity 36)
    pub name: String,

    /// Operator-defined label propagated from the EdgionPlugins entry's
    /// `alias` field. Independent of `name`. Stored separately from the
    /// 128-byte LogBuffer so it never reduces buffer space available to
    /// `push` / `push_err`.
    pub alias: Option<String>,

    /// Time cost in microseconds (us), None if not measured
    pub time_cost: Option<u64>,

    /// Condition skip reason, None if not skipped
    /// e.g., "skip:keyExist,hdr:X-Internal"
    pub cond_skip: Option<String>,

    /// Tier-1 fixed 128-byte stack buffer — the common, zero-heap path.
    /// Serialized merged into the `"log"` array, not as its own field.
    pub log: Option<LogBuffer>,

    /// Tier-2 heap overflow buffer, lazily created only when tier-1 fills.
    /// Serialized merged into the `"log"` array after the tier-1 entries —
    /// the stack/heap tier split is an internal implementation detail.
    pub ulog: Option<ULogBuffer>,

    /// Set when an entry was dropped because even the tier-2 `ulog` overflow
    /// buffer was full (only serialized when true).
    pub log_full: Option<bool>,

    /// ExtensionRef reference name (for ExtensionRef filter only)
    pub refer_to: Option<String>,

    /// Request-local attachment token. Never serialized.
    pub(crate) internal_token: Option<String>,
}

impl Serialize for PluginLog {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;

        // Tier-1 `log` and tier-2 `ulog` serialize as ONE merged `"log"` array
        // in push order; `ulog` is never emitted as its own field.
        let has_log = self.log.is_some() || self.ulog.is_some();
        let len = 1
            + usize::from(self.alias.is_some())
            + usize::from(self.time_cost.is_some())
            + usize::from(self.cond_skip.is_some())
            + usize::from(has_log)
            + usize::from(self.log_full.is_some())
            + usize::from(self.refer_to.is_some());

        let mut st = serializer.serialize_struct("PluginLog", len)?;
        st.serialize_field("name", &self.name)?;
        if let Some(ref alias) = self.alias {
            st.serialize_field("alias", alias)?;
        }
        if let Some(time_cost) = self.time_cost {
            st.serialize_field("time_cost", &time_cost)?;
        }
        if let Some(ref cond_skip) = self.cond_skip {
            st.serialize_field("cond_skip", cond_skip)?;
        }
        if has_log {
            st.serialize_field(
                "log",
                &MergedLog {
                    log: self.log.as_ref(),
                    ulog: self.ulog.as_ref(),
                },
            )?;
        }
        if let Some(log_full) = self.log_full {
            st.serialize_field("log_full", &log_full)?;
        }
        if let Some(ref refer_to) = self.refer_to {
            st.serialize_field("refer_to", refer_to)?;
        }
        st.end()
    }
}

/// Serializes a `PluginLog`'s two physical buffers as one flat JSON array of
/// entries — tier-1 `log` entries followed by tier-2 `ulog` overflow entries,
/// preserving global push order.
struct MergedLog<'a> {
    log: Option<&'a LogBuffer>,
    ulog: Option<&'a ULogBuffer>,
}

impl Serialize for MergedLog<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let log = self.log.into_iter().flat_map(|b| b.entries());
        let ulog = self.ulog.into_iter().flat_map(|b| b.entries());
        serializer.collect_seq(log.chain(ulog))
    }
}

impl PluginLog {
    /// Allocation-free scratch value for plugins that own a dedicated
    /// top-level access-log block and suppress the generic stage entry.
    #[inline]
    pub(crate) const fn disabled() -> Self {
        Self {
            name: String::new(),
            alias: None,
            time_cost: None,
            cond_skip: None,
            log: None,
            ulog: None,
            log_full: None,
            refer_to: None,
            internal_token: None,
        }
    }

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
            internal_token: None,
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

    /// Route one entry through the two-tier buffer.
    ///
    /// Tries tier-1 `log` first. On overflow it lazily opens tier-2 `ulog` and
    /// writes the entry there instead. Once `ulog` exists every subsequent entry
    /// goes straight to `ulog`, so global push order is always preserved. When
    /// even `ulog` is full the entry is dropped and `log_full` is set.
    ///
    /// The stack/heap tier split is internal: both buffers serialize merged into
    /// one `"log"` array (see `impl Serialize for PluginLog`).
    ///
    /// `to_log` / `to_ulog` are the same logical write expressed against each
    /// buffer type (e.g. `push` vs `push`, or `push_err` vs `push_err`), so the
    /// zero-allocation `LogBuffer::push_err` fast path is preserved on tier 1.
    #[inline]
    fn push_via(
        &mut self,
        to_log: impl FnOnce(&mut LogBuffer) -> bool,
        to_ulog: impl FnOnce(&mut ULogBuffer) -> bool,
    ) -> bool {
        if self.ulog.is_none() {
            if to_log(self.log.get_or_insert_with(LogBuffer::new)) {
                return true;
            }
            // Tier-1 full: lazily open the tier-2 overflow buffer.
            self.ulog.get_or_insert_with(ULogBuffer::new);
        }
        let ok = to_ulog(self.ulog.as_mut().expect(
            "ulog is Some: either pre-existing (if branch skipped) or initialized by get_or_insert_with above",
        ));
        if !ok {
            self.log_full = Some(true);
        }
        ok
    }

    /// Push to fixed buffer (recommended for non-error paths).
    ///
    /// For error entries, use [`PluginLog::push_err`] — it enforces a
    /// `&'static str` argument and unified `"err: "` prefix, blocking
    /// accidental token/claim leakage via `format!` or error `Display`.
    #[inline]
    pub fn push(&mut self, log: &str) -> bool {
        self.push_via(|b| b.push(log), |u| u.push(log))
    }

    /// Push an error entry with unified `"err: "` prefix.
    ///
    /// `kind` is `&'static str` so the compiler rejects `format!` / `err.to_string()`
    /// at the call site — the only way to include dynamic detail is via `ctx.err_log`
    /// (access log) after explicit redaction. This is the required API for all new
    /// plugin error logging; `push(&format!("...: {}", err))` is forbidden for error
    /// paths because error `Display` may carry tokens, claims, or raw user input.
    ///
    /// Serialized entry: `"err: <kind>"`. Plugin identity comes from
    /// [`PluginLog::name`], so `kind` should be the event only (e.g.
    /// `"token refresh fail"`), not `"<plugin>:<event>"`.
    ///
    /// Format spec + examples: `skills/03-coding/observability/00-access-log.md`
    /// (section "Error entry API constraints"). Safety rationale: `skills/03-coding/01-log-safety.md`.
    #[inline]
    pub fn push_err(&mut self, kind: &'static str) -> bool {
        self.push_via(|b| b.push_err(kind), |u| u.push_err(kind))
    }

    /// Push a user-controlled dynamic value through the active masking mode.
    ///
    /// Truncates to the last UTF-8 char boundary at or before 64 bytes to respect
    /// the tier-1 LogBuffer's 128-byte limit after any wrapper overhead.
    /// Use this for DSL/plugin content that may contain credentials or PII.
    /// For static event labels, use [`push_err`] instead.
    #[inline]
    pub fn push_sensitive(&mut self, value: &str) -> bool {
        // Reserve bytes for wrap prefix+suffix so the final entry fits the tier-1
        // 128-byte LogBuffer. Default wrap overhead is 7 bytes ("<E>" + "</E>");
        // operators can configure longer strings.
        let overhead = edgion_resources::sensitive::wrap_overhead();
        let max_value_bytes = (128usize.saturating_sub(overhead)).min(64);
        let end = (0..=value.len().min(max_value_bytes))
            .rev()
            .find(|&i| value.is_char_boundary(i))
            .unwrap_or(0);
        let truncated = &value[..end];
        edgion_resources::sensitive::apply_sensitive(truncated, |w| self.push_via(|b| b.push(w), |u| u.push(w)))
    }

    /// Push an error entry whose value is dynamic — atomically writes
    /// `"err: <masked-value>"` as a single buffer entry.
    ///
    /// The dynamic counterpart of [`push_err`] for callers that cannot supply a
    /// `&'static str` (notably the DSL `log_error()` builtin). Output format
    /// matches [`push_err`] so downstream parsers grepping for `err:` see both.
    /// The value is masked through the active sensitive mode and truncated to
    /// fit inside the LogBuffer entry after accounting for the prefix and any
    /// wrap overhead.
    pub fn push_err_sensitive(&mut self, value: &str) -> bool {
        const PREFIX: &str = "err: ";
        let overhead = edgion_resources::sensitive::wrap_overhead();
        let max_value_bytes = (128usize.saturating_sub(PREFIX.len() + overhead)).min(64);
        let end = (0..=value.len().min(max_value_bytes))
            .rev()
            .find(|&i| value.is_char_boundary(i))
            .unwrap_or(0);
        let truncated = &value[..end];
        let mut entry = String::with_capacity(PREFIX.len() + max_value_bytes + overhead);
        entry.push_str(PREFIX);
        edgion_resources::sensitive::apply_sensitive(truncated, |w| entry.push_str(w));
        self.push_via(|b| b.push(&entry), |u| u.push(&entry))
    }

    /// Push a format-string log entry where only explicitly marked args are masked.
    ///
    /// The template string is never masked. Each `{}` placeholder is replaced by the
    /// corresponding [`LogArg`]: `Plain` args are inserted verbatim, `Unsigned` args
    /// use decimal digits, and `Sensitive` args are
    /// passed through the active masking mode (replace/wrap). Use the [`la!`] macro to
    /// build the args slice conveniently.
    ///
    /// ```ignore
    /// log.push_fmt("key {} set", la![rule.name.as_str()]);
    /// log.push_fmt("user {} from {}", la!["admin", sensitive(&remote_ip)]);
    /// ```
    pub fn push_fmt(&mut self, fmt: &str, args: &[LogArg<'_>]) -> bool {
        // 128-byte inline scratch matches the tier-1 LogBuffer budget, so a
        // tier-1-sized entry never heap-allocates here.
        let mut buf: SmallVec<[u8; 128]> = SmallVec::new();
        if !format_into(&mut buf, fmt, args) {
            // Preserve the same tier transition as an oversized plain entry,
            // without retaining any partially rendered content.
            return self.push_via(|_| false, |_| false);
        }
        let s = std::str::from_utf8(&buf).unwrap_or("");
        self.push_via(|b| b.push(s), |u| u.push(s))
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

    pub(crate) fn set_internal_token(&mut self, token: String) {
        self.internal_token = Some(token);
    }

    /// Render this log as a single `String` suitable for `expect-test`
    /// snapshot assertions.
    ///
    /// Entries inside one tier are joined by `\n`. When both tier-1 (`log`)
    /// and tier-2 (`ulog`) carry entries, they are separated by the literal
    /// marker `\n--- \n` — a sequence reviewers can grep for and visually
    /// distinguish in `git diff`. Empty `PluginLog` yields `""`.
    ///
    /// `#[cfg(test)]` is deliberate: this format is for tests only. A
    /// production `Display` impl is intentionally NOT provided to prevent
    /// accidental `println!("{}", plugin_log)` from leaking into hot paths.
    ///
    /// See `docs/superpowers/specs/2026-05-27-snapshot-testing-skill-and-pilot-design.md`
    /// § 2.2 for the separator-format decision rationale.
    #[cfg(test)]
    pub fn render_for_test(&self) -> String {
        let main: Vec<&str> = self.log.iter().flat_map(|b| b.entries()).collect();
        let user: Vec<&str> = self.ulog.iter().flat_map(|b| b.entries()).collect();
        match (main.is_empty(), user.is_empty()) {
            (true, true) => String::new(),
            (false, true) => main.join("\n"),
            (true, false) => user.join("\n"),
            (false, false) => {
                format!("{}\n--- \n{}", main.join("\n"), user.join("\n"))
            }
        }
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

/// Stage logs for a specific execution stage
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
#[path = "../../../tests/unit/plugins/runtime/log_test.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../tests/unit/plugins/runtime/log_test/render_for_test_tests.rs"]
mod render_for_test_tests;
