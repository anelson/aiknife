pub mod audio;
pub mod chat;
mod error;
mod tokenize;
mod util;

pub use error::Result;

#[cfg(test)]
pub mod test_helpers {
    use std::sync::OnceLock;
    use tracing::level_filters::LevelFilter;
    use tracing_subscriber::{fmt::format::FmtSpan, EnvFilter};

    static TRACING: OnceLock<()> = OnceLock::new();

    /// Initialize tracing for tests with a stdout subscriber.
    /// Safe to call multiple times - will only initialize once.
    pub fn init_test_logging() {
        TRACING.get_or_init(|| {
            tracing_subscriber::fmt()
                .with_env_filter(
                    EnvFilter::builder()
                        .with_default_directive(LevelFilter::DEBUG.into())
                        .from_env_lossy(),
                )
                .with_span_events(FmtSpan::CLOSE | FmtSpan::NEW)
                .with_test_writer()
                .try_init()
                .ok();
        });
    }
}
