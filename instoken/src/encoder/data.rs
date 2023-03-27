//! instoken embeds the token lists (also called "mergeable ranks" in the tiktoken code) into the
//! Rust binary in release builds.  In debug builds the files are simply loaded from the local
//! filesystem for a faster experience while iteratively developing.
//!
//! If you're the kind of Rust programmer who agonizes over the size of your binaries, you
//! definitely don't want to use instoken.  Then again that means you'll need to use tiktoken which
//! will drag along a dependency on the fucking Python runtime, so perhaps it's time to get off
//! that high horse and accept that this data is going to end up on your computer taking up space
//! on way or another, so it may as well be in the executable image where it's most convenient.
use crate::EncoderType;
use base64::{engine::general_purpose, Engine as _};
use rust_embed::RustEmbed;
use std::io::{BufRead, BufReader};

#[derive(RustEmbed)]
#[folder = "tokens/cl100k_base"]
struct Cl100kBase;

#[derive(RustEmbed)]
#[folder = "tokens/gpt-2"]
struct Gpt2;

#[derive(RustEmbed)]
#[folder = "tokens/p50k_base"]
struct P50kBase; // NOTE: p50k_edit and p50k_base share the same tokens

#[derive(RustEmbed)]
#[folder = "tokens/r50k_base"]
struct R50kBase;

// TODO: expose these as functions instead of just the raw data

/// Load the list of subword tokens and their token rank for a particular tokenizer.
///
/// These are compiled in to the crate so this operation should be infallible unless the embedded
/// data is somehow invalid.  In that case this code will panic.
///
/// The data is represented as an iterator that yields at tuple of `(bytes, token)`.  This can then
/// be processed in whatever way the caller requires.  Typically it's made into a hash map but
/// other more exotic structures are also possible.
pub fn get_token_data(tokenizer: EncoderType) -> impl Iterator<Item = (Vec<u8>, usize)> {
    match tokenizer {
        EncoderType::Cl100kBase => load_data(Cl100kBase::get("cl100k_base.tiktoken")),
        EncoderType::Gpt2 => todo!("TODO: GPT-2 requires processing two files into mergeable ranks.  Do that if this is important"),
        EncoderType::P50kBase | EncoderType::P50kEdit => load_data(P50kBase::get("p50k_base.tiktoken")),
        EncoderType::R50kBase => load_data(R50kBase::get("r50k_base.tiktoken")),
    }
}

fn load_data(
    rust_embed: Option<rust_embed::EmbeddedFile>,
) -> impl Iterator<Item = (Vec<u8>, usize)> {
    let rust_embed = rust_embed.expect("BUG: Required embedded data is missing");

    let text = String::from_utf8_lossy(rust_embed.data.as_ref());

    text.lines()
        .map(move |line| {
            // Each line is of the form "<base64 string> <rank>"
            let (bytes, rank) = line.split_once(' ').expect("BUG: Invalid embedded data");

            let bytes = general_purpose::STANDARD
                .decode(bytes)
                .unwrap_or_else(|e| panic!("Error decoding base64 value `{bytes}`: {e}"));
            let rank = rank
                .parse::<usize>()
                .expect("BUG: Embedded data has non-integer rank value");

            (bytes, rank)
        })
        .collect::<Vec<_>>()
        .into_iter()
}
