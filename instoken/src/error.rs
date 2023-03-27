use snafu::Snafu;
use std::path::PathBuf;

#[derive(Debug, Snafu)]
#[snafu(visibility(pub(crate)))]
pub enum InstokenError {
    #[snafu(display("The encoding '{encoding} isn't one of the supported encodings"))]
    UnknownEncoding { encoding: String },

    #[snafu(display("File I/O error on file '{}'", path.display()))]
    FileIo {
        path: PathBuf,
        source: std::io::Error,
    },
}
