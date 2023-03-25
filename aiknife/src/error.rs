use std::path::PathBuf;

use snafu::Snafu;

pub type Result<T, E = AiKnifeError> = std::result::Result<T, E>;

#[derive(Debug, Snafu)]
#[snafu(visibility(pub(crate)))]
pub enum AiKnifeError {
    /// Regretably, tiktoken-rs uses a very unfortunate anti-pattern, of the `anyhow` crate as the
    /// way to represent library errors.  `anyhow::Error` doesn't implement `std::error::Error` and
    /// thus, ironically, can't be used as the error source.
    #[snafu(display("tiktoken-rs error: {inner:#}"))]
    TikTokenRs { inner: anyhow::Error },

    #[snafu(display("Async file I/O error on file '{}'", path.display()))]
    AsyncFileIo {
        path: PathBuf,
        source: tokio::io::Error,
    },

    #[snafu(display("File I/O error on file '{}'", path.display()))]
    FileIo {
        path: PathBuf,
        source: std::io::Error,
    },
}
