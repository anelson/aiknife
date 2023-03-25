//! Tokenization functions which process text into tokens suitable for use with OpenAI APIs
use crate::Result;
use futures::{Stream, StreamExt};
use snafu::ResultExt;
use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt::Display;
use std::io::Read;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::AsyncReadExt;
use tracing::*;

pub type Tokenizer = tiktoken_rs::tokenizer::Tokenizer;
pub type Encoder = tiktoken_rs::CoreBPE;

pub const TOKENIZERS: &[Tokenizer] = &[
    Tokenizer::Cl100kBase,
    Tokenizer::P50kBase,
    Tokenizer::R50kBase,
    Tokenizer::P50kEdit,
    Tokenizer::Gpt2,
];

/// Get the `tiktokrne-rs` encoder that corresponds to the tokenizing algorithm used by OpenAI for
/// ChatGPT models and `text-embedding-ada-002`
fn cl100k_base_encoder() -> Result<Encoder> {
    get_encoder_for_tokenizer(tiktoken_rs::tokenizer::Tokenizer::Cl100kBase)
}

pub fn get_tokenizer_name(tokenizer: Tokenizer) -> &'static str {
    match tokenizer {
        tiktoken_rs::tokenizer::Tokenizer::Cl100kBase => "cl100k_base",
        tiktoken_rs::tokenizer::Tokenizer::P50kBase => "p50k_base",
        tiktoken_rs::tokenizer::Tokenizer::R50kBase => "r50k_base",
        tiktoken_rs::tokenizer::Tokenizer::P50kEdit => "p50k_edit",
        tiktoken_rs::tokenizer::Tokenizer::Gpt2 => "gpt2",
    }
}

pub fn get_encoder_for_tokenizer(tokenizer: Tokenizer) -> Result<Encoder> {
    tiktoken_rs::get_bpe_from_tokenizer(tokenizer)
        .map_err(|e| crate::error::TikTokenRsSnafu { inner: e }.build())
}

/// A token produced by the tokenizer.
///
/// Tokens are stored in an efficient internal format that isn't immediately usable as text.
#[derive(Debug, Clone)]
pub struct Token {
    /// The internal representation of the token in BPE form
    pub integer: usize,

    /// The original text
    ///
    /// TODO: this is very wasteful.  The original `Vec<u8>` is one of a finite set of possible
    /// sequences, the mapping for which is  stored in the internal state of the `CoreBPE` type.
    /// If that were exposed in the public API, we could look it up on the fly and avoid a heap
    /// allocation here.
    pub original: Option<String>,
}

impl Token {
    fn new(token: usize, encoder: &tiktoken_rs::CoreBPE) -> Self {
        let original = match encoder.decode(vec![token]) {
            Ok(string) => Some(string),
            Err(e) => {
                error!(err = e.to_string(),
                            token,
                            "Decoding token into a string failed.  Probably this token isn't on a UTF-8 boundary");
                None
            }
        };

        Self {
            integer: token,
            original: None,
        }
    }
}

impl Display for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "TODO! {}", self.integer)
    }
}

/// A single token from a file.
#[derive(Debug, Clone)]
pub struct FileToken {
    /// The path of the file that this token came from
    pub path: Arc<PathBuf>,

    /// The byte range within the file that these tokens come from
    pub range: Range<u64>,

    /// The token itself
    pub token: Arc<Token>,
}

/// Tokenize one or more files in parallel.
///
/// If multiple `files` are specified, they will be tokenized in parallel, up to the maximum number
/// of parallel CPUs on the system.
///
/// The result is a `Stream` that yields the results of tokenizing each file, in a random order.
pub async fn tokenize_files_streaming<I, P>(
    tokenizer: Tokenizer,
    files: I,
) -> Result<impl Stream<Item = Result<(Arc<PathBuf>, Vec<FileToken>)>>>
where
    I: IntoIterator<Item = P>,
    P: Into<PathBuf>,
{
    // TODO: support globs and directory names here, expanding them into a list of all files to
    // process

    let tokenize_futs = files
        .into_iter()
        .map(move |file| tokenize_file(tokenizer, file.into()));

    // Make a stream from this iterator of futs
    let tokenize_stream = futures::stream::iter(tokenize_futs);

    // Process the stream in parallel up to some max paralelism
    // TODO: parameterize this and default to the number of CPU cores in the system
    let results = tokenize_stream.buffer_unordered(4);

    Ok(results)
}

/// Tokenize one or more files in parallel.
///
/// If multiple `files` are specified, they will be tokenized in parallel, up to the maximum number
/// of parallel CPUs on the system.
///
/// Wraps [`tokenize_files_streaming`] and doesn't return until all files have been successfully
/// processed
pub async fn tokenize_files<I, P>(
    tokenizer: Tokenizer,
    files: I,
) -> Result<Vec<Result<(Arc<PathBuf>, Vec<FileToken>)>>>
where
    I: IntoIterator<Item = P>,
    P: Into<PathBuf>,
{
    let stream = tokenize_files_streaming(tokenizer, files).await?;

    Ok(stream.collect().await)
}

pub async fn tokenize_stream(tokenizer: Tokenizer, stream: impl Read) -> Result<()> {
    todo!()
}

/// Tokenize a single file.
///
/// This function can be called on its own when you know you have only one file to tokenize,
/// however if you need to tokenize multiple files either [`tokenize_files`] or
/// [`tokenize_files_streaming`] will be potentically much more performant.
pub async fn tokenize_file(
    tokenizer: Tokenizer,
    path: impl Into<PathBuf>,
) -> Result<(Arc<PathBuf>, Vec<FileToken>)> {
    let path = path.into();

    // TODO: this implements `Clone`, is it very expensive to create?  It looks like it contains
    // RegEx objects, so it might be worthwhile to create this one and then clone it as needed when
    // handling multiple files in a batch.
    let encoder = get_encoder_for_tokenizer(tokenizer)?;

    let mut file = tokio::fs::File::open(&path)
        .await
        .with_context(|_| crate::error::FileIoSnafu { path: path.clone() })?;
    let metadata = file
        .metadata()
        .await
        .with_context(|_| crate::error::FileIoSnafu { path: path.clone() })?;
    let mut contents = Vec::with_capacity(metadata.len() as usize);
    file.read_to_end(&mut contents)
        .await
        .with_context(|_| crate::error::FileIoSnafu { path: path.clone() })?;

    // The file might or might not be valid Unicode text.  If it contains some invalid Unicode code
    // points, rather than fail the encoding, just substitute them
    //
    // TODO: support other encodings
    let contents = String::from_utf8_lossy(&contents);

    if let Cow::Owned(_) = &contents {
        // The input string wasn't entirely valid UTF-8, so the invalid bytes were replaced with a
        // placeholder.  This means the tokenization isn't going to be precisely matching the input
        // file
        warn!(path = %path.display(),
            "Input file did not decode as clean UTF-8.   \
            Invalid bytes have been replaced with a UTF-8 placeholder sequence.   \
            The resulting tokens will not be able to precisely reproduce this file");
    }

    // We will store unique tokens once, to avoid the excessive memory allocation they otherwise
    // cause
    //
    // Estimate the number of tokens with the following assumptions, which may or may not actually
    // hold:
    // - Approximately one token per 4 bytes
    // - The document contains unique tokens (this is definitely not the case but let's assume)
    // - The max number of unique tokens in any given file or document is about 64K.  This is based
    // on nothing other than a wild guess.  If files/docs tend to have orders of magnitude more
    // tokens than this, then the hashmap reallocating itself will hurt performance.
    //
    // Based on this reserve heap memory for this hash map accordingly.  It's okay to over-allocate
    // memory, that's not as expensive as multiple re-allocations of the hash map
    let mut tokens = HashMap::with_capacity(std::cmp::min(contents.len() / 4, u16::MAX as usize));

    let path = Arc::new(path);

    let tokens = encoder
        .encode_with_special_tokens(&contents)
        .into_iter()
        .scan(0u64, |last_offset, token| {
            // If this token hasn't been seen yet during this tokenization step, make a new one,
            // otherwise make a cheap clone of the one we already saw.
            //
            // This wouldn't be necessary except for the ugly hack explained on Token::original.
            let token = tokens
                .entry(token)
                .or_insert_with(|| Arc::new(Token::new(token, &encoder)))
                .clone();

            // Use the length of this token and the offset in the file where this occurs to
            // calculate the range within the file where this token appears

            // TODO: can't compute length until we can 100% reliably get the original bytes back
            // from the tokens
            //let length = token.original.as_bytes().len();
            let length = 0;

            let next_offset = *last_offset + length as u64;
            let range = *last_offset..next_offset;

            *last_offset = next_offset;

            Some(FileToken {
                path: path.clone(),
                range,
                token,
            })
        })
        .collect();

    Ok((path, tokens))
}

#[cfg(test)]
mod tests {
    use std::fs::File;
    use std::io::{BufRead, BufReader};

    use walkdir::WalkDir;

    use super::*;

    /// For each of the files in `test_data/input`, we have pre-computed expected tokens from the
    /// official `tiktoken` OpenAI github repo implementation.  Tokenize each of those files into
    /// tokens, and verify they match the expected results
    #[tokio::test]
    async fn test_tokenize_files() {
        const INPUT_DIRECTORY: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/test_data/input");
        const OUTPUT_DIRECTORY: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/test_data/output");

        for tokenizer in TOKENIZERS {
            println!("Testing tokenizer: {}", get_tokenizer_name(*tokenizer));

            for entry in WalkDir::new(INPUT_DIRECTORY)
                .into_iter()
                .filter_map(|e| e.ok())
            {
                // Skip directories or files that are not regular files
                if !entry.file_type().is_file() {
                    continue;
                }

                // Get the full path of the input file
                let input_file_path = entry.path();

                println!("Testing input file {}", input_file_path.display());

                // Find and load the known answer file for this tokenizer.  It consists of
                // one token per line, represented as a base 10 integer
                let known_answer_file = Path::new(OUTPUT_DIRECTORY)
                    .join(get_tokenizer_name(*tokenizer))
                    .join(input_file_path.file_name().unwrap());

                let expected_tokens = {
                    let mut tokens = Vec::new();
                    let file = File::open(&known_answer_file).unwrap();
                    let reader = BufReader::new(file);
                    for line in reader.lines() {
                        let line = line.unwrap();
                        tokens.push(line.trim().parse::<usize>().unwrap());
                    }
                    tokens
                };

                let (_path, tokens) = tokenize_file(*tokenizer, input_file_path).await.unwrap();

                let integer_tokens: Vec<_> = tokens
                    .iter()
                    .map(|file_token| file_token.token.integer)
                    .collect();

                if expected_tokens != integer_tokens {
                    // Test failure.  But we need a more actionable failure message.

                    for (index, (expected, actual)) in expected_tokens
                        .iter()
                        .zip(integer_tokens.iter())
                        .enumerate()
                    {
                        assert_eq!(
                            expected,
                            actual,
                            "Tokenizer {}, input file {}, token index {index} does not match",
                            get_tokenizer_name(*tokenizer),
                            input_file_path.display()
                        );
                    }

                    // If execution made if this far, then the problem is more tokens in one result
                    // than the other
                    assert_eq!(
                        expected_tokens.len(),
                        integer_tokens.len(),
                        "Tokenizer {}, input file {} resulting token counts don't match",
                        get_tokenizer_name(*tokenizer),
                        input_file_path.display()
                    );
                }
            }
        }
    }
}
