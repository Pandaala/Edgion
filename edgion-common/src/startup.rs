use tracing_subscriber::EnvFilter;

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
        // Fallback for panics before the subscriber is installed (tracing is a
        // no-op then). Harmless duplication once tracing also writes to stderr.
        eprintln!("[panic] thread '{thread}' panicked at {location}: {message}\n{backtrace}");
    }));
}

pub fn init_default_tracing() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("info".parse().unwrap()))
        .init();
}
