use tracing_subscriber::{fmt, EnvFilter};

/// Initialize structured logging with tracing.
/// Set RUST_LOG environment variable to control log levels.
/// e.g., RUST_LOG=arb_bot=debug,info
pub fn init_logging() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("arb_bot=info,warn"));

    fmt()
        .with_env_filter(filter)
        .with_target(true)
        .with_thread_ids(true)
        .with_file(true)
        .with_line_number(true)
        .compact()
        .init();
}
