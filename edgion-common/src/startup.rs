use std::sync::atomic::{AtomicBool, Ordering};

/// Whether a `tracing` subscriber has been installed. Flipped to `true` by
/// `mark_tracing_ready()` (called from `init_logging` once the subscriber is
/// live). Before that point `tracing::*` macros are silent no-ops, so the
/// startup-error and panic paths fall back to a plain stderr line; afterwards
/// they route through structured logging instead of bypassing the configured
/// log sink.
static TRACING_READY: AtomicBool = AtomicBool::new(false);

/// Mark the `tracing` subscriber as installed. Call once, right after the
/// subscriber is initialized.
pub fn mark_tracing_ready() {
    TRACING_READY.store(true, Ordering::SeqCst);
}

/// Returns whether the `tracing` subscriber has been installed.
pub fn tracing_ready() -> bool {
    TRACING_READY.load(Ordering::SeqCst)
}

/// Fallback reporter for a fatal top-level error in a service binary's `main`,
/// for the window *before* structured logging is live.
///
/// Contract: once the subscriber is installed, the service's `run()` is
/// responsible for logging the error through `tracing` *while the log
/// `WorkerGuard` is still alive*. Writing here in that case would be too late --
/// the non-blocking file worker shuts down with the guard, so a `tracing::error!`
/// emitted from `main` (after `run` returned and the guard dropped) would be
/// silently discarded. Therefore this only emits when logging is not yet ready
/// (e.g. a config-load failure before `init_logging`), where `tracing` is a
/// no-op and stderr is the only diagnostic channel. The alternate `{:#}`
/// formatting expands the full error chain.
pub fn report_startup_failure(err: &(impl std::fmt::Display + ?Sized)) {
    if !tracing_ready() {
        eprintln!("[startup] fatal error: {err:#}");
    }
}

pub fn init_crypto() {
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Failed to install rustls crypto provider");
}

/// Install a process-wide panic hook that routes panics through `tracing`
/// (structured, `component = "panic"`) with a captured backtrace, instead of the
/// default stderr-only handler.
///
/// The backtrace is always captured (`force_capture`) regardless of
/// `RUST_BACKTRACE`, so panics are diagnosable in logs; readable frame names
/// require the binary to keep its symbols (production images ship unstripped).
/// A plain stderr line is also emitted as a fallback for panics that occur
/// before the tracing subscriber is installed (where `tracing::error!` is a
/// silent no-op). Call once, as early as possible in each binary's `main`.
pub fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "<unknown>".to_string());
        let payload = info.payload();
        let message = payload
            .downcast_ref::<&str>()
            .map(|s| (*s).to_string())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "<non-string panic payload>".to_string());
        let thread = std::thread::current().name().unwrap_or("<unnamed>").to_string();
        let backtrace = std::backtrace::Backtrace::force_capture();

        tracing::error!(
            component = "panic",
            location = %location,
            thread = %thread,
            backtrace = %backtrace,
            "thread panicked: {}",
            message
        );
        if tracing_ready() {
            // Subscriber is live: the full message + backtrace already went to
            // structured logging above. Keep stderr to a single generic line so
            // panic internals (paths, topology, payload fragments) do not leak
            // outside the configured log sink.
            eprintln!("[panic] thread '{thread}' panicked at {location}; see structured logs for details");
        } else {
            // Subscriber not yet installed (the tracing::error! above was a
            // no-op): stderr is the only diagnostic channel for very early
            // panics, so emit the full detail here as a fallback.
            eprintln!("[panic] thread '{thread}' panicked at {location}: {message}\n{backtrace}");
        }
    }));
}
