pub mod chat;
pub mod audio;
mod error;
mod tokenize;
mod util;

pub use error::Result;

#[cfg(test)]
pub mod test_helpers {
    use std::sync::OnceLock;
    use tracing_subscriber::{fmt::format::FmtSpan, EnvFilter};

    static TRACING: OnceLock<()> = OnceLock::new();

    /// Initialize tracing for tests with a stdout subscriber.
    /// Safe to call multiple times - will only initialize once.
    pub fn init_test_logging() {
        TRACING.get_or_init(|| {
            let filter = std::env::var("RUST_LOG")
                .map(EnvFilter::new)
                .unwrap_or_else(|_| EnvFilter::new("debug"));

            tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_span_events(FmtSpan::CLOSE)
                .with_test_writer()
                .try_init()
                .ok();
        });
    }
}

