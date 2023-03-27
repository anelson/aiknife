use crate::{TokenInt, TokenString};
use once_cell::sync::OnceCell;
use std::sync::Arc;
use strum::{EnumIter, EnumString, EnumVariantNames, IntoEnumIterator};

mod data;
mod hash;

/// The tokenizer encoding to use to tokenize text.
///
/// Each of these uses a BPE subword tokenizing approach, but with different sets of
/// tokens and corresponding ranks, and with a different regex for breaking up text into
/// approximate word boundaries.
#[derive(
    Clone, Copy, Debug, Eq, PartialEq, EnumString, EnumIter, EnumVariantNames, strum::Display,
)]
pub enum EncoderType {
    #[strum(serialize = "cl100k_base")]
    Cl100kBase,
    #[strum(serialize = "gpt-2")]
    Gpt2,
    #[strum(serialize = "p50k_base")]
    P50kBase,
    #[strum(serialize = "p50k_edit")]
    P50kEdit,
    #[strum(serialize = "r50k_base")]
    R50kBase,
}

/// The description of a particular BPE encoding scheme
pub struct BpeEncoder {
    pub(crate) typ: EncoderType,

    /// Mapping of byte sequences to integer token ranks
    pub(crate) encode: hash::TokenEncoder,

    /// Like [`encode`] but for "special" tokens.
    ///
    /// These should not be present in `encode`
    pub(crate) special_tokens_encode: hash::TokenEncoder,

    /// Mapping of integer token ranks back to byte sequences
    pub(crate) decode: hash::TokenDecoder,

    /// Like [`decode`] but for "special" tokens.
    ///
    /// These should not be present in `decode`
    pub(crate) special_tokens_decode: hash::TokenDecoder,

    /// Regex used to break up text into approximate word boundaries
    ///
    /// Regrettably, the word separator regexes used by many OpenAI models' tokenizers uses "fancy"
    /// features that are not availablein the regular rust `regex` crate.  So this slower 'fancy'
    /// impl is needed
    ///
    /// TODO: would it be faster to simply re-implement the "fancy" regex logic in Rust code?
    pub(crate) regex: fancy_regex::Regex,

    /// All token strings, sorted lexicographically
    pub(crate) sorted_token_bytes: Vec<TokenString>,
}

impl BpeEncoder {
    /// Load the parameters for the given encoder type, if not already present.
    ///
    /// Each encoder type is lazily loaded on demand, but only once.  After that the loaded encoder
    /// is held in memory for the duration of the process.  The resulting `Arc` is very cheap to
    /// clone.
    pub fn new(typ: EncoderType) -> Arc<Self> {
        // The following was adapted from the python `openai_public.py` from the `tiktokens` source
        const ENDOFTEXT: &str = "<|endoftext|>";
        const FIM_PREFIX: &str = "<|fim_prefix|>";
        const FIM_MIDDLE: &str = "<|fim_middle|>";
        const FIM_SUFFIX: &str = "<|fim_suffix|>";
        const ENDOFPROMPT: &str = "<|endofprompt|>";

        match typ {
            EncoderType::Cl100kBase => {
                static INSTANCE: OnceCell<Arc<BpeEncoder>> = OnceCell::new();
                INSTANCE.get_or_init(|| {
                    let data = data::get_token_data(typ);
                    let special_tokens = vec![(ENDOFTEXT, 100257usize), (FIM_PREFIX, 100258), (FIM_MIDDLE, 100259), (FIM_SUFFIX, 100260), (ENDOFPROMPT, 100276)];
                    const RE: &str = r##"(?i:'s|'t|'re|'ve|'m|'ll|'d)|[^\r\n\p{L}\p{N}]?\p{L}+|\p{N}{1,3}| ?[^\s\p{L}\p{N}]+[\r\n]*|\s*[\r\n]+|\s+(?!\S)|\s+"##;

                    Arc::new(BpeEncoder::new_internal(typ, data, special_tokens.into_iter(),RE))
                }).clone()
            }
            EncoderType::Gpt2 => {
                static INSTANCE: OnceCell<Arc<BpeEncoder>> = OnceCell::new();
                INSTANCE.get_or_init(|| {
                    let data = data::get_token_data(typ);
                    let special_tokens = vec![(ENDOFTEXT, 50256usize)];
                    const RE: &str = r##"'s|'t|'re|'ve|'m|'ll|'d| ?\p{L}+| ?\p{N}+| ?[^\s\p{L}\p{N}]+|\s+(?!\S)|\s+"##;

                    Arc::new(BpeEncoder::new_internal(typ, data, special_tokens.into_iter(),RE))
                }).clone()
            }
            EncoderType::P50kBase => {
                static INSTANCE: OnceCell<Arc<BpeEncoder>> = OnceCell::new();
                INSTANCE.get_or_init(|| {
                    let data = data::get_token_data(typ);
                    let special_tokens = vec![(ENDOFTEXT, 50256usize)];
                    const RE: &str = r##"'s|'t|'re|'ve|'m|'ll|'d| ?\p{L}+| ?\p{N}+| ?[^\s\p{L}\p{N}]+|\s+(?!\S)|\s+"##;

                    Arc::new(BpeEncoder::new_internal(typ, data, special_tokens.into_iter(),RE))
                }).clone()
            }
            EncoderType::P50kEdit => {
                static INSTANCE: OnceCell<Arc<BpeEncoder>> = OnceCell::new();
                INSTANCE.get_or_init(|| {
                    let data = data::get_token_data(typ);
                    let special_tokens = vec![(ENDOFTEXT, 50256usize), (FIM_PREFIX, 50281), (FIM_MIDDLE, 50282), (FIM_SUFFIX, 50283)];
                    const RE: &str = r##"'s|'t|'re|'ve|'m|'ll|'d| ?\p{L}+| ?\p{N}+| ?[^\s\p{L}\p{N}]+|\s+(?!\S)|\s+"##;

                    Arc::new(BpeEncoder::new_internal(typ, data, special_tokens.into_iter(),RE))
                }).clone()
            }
            EncoderType::R50kBase => {
                static INSTANCE: OnceCell<Arc<BpeEncoder>> = OnceCell::new();
                INSTANCE.get_or_init(|| {
                    let data = data::get_token_data(typ);
                    let special_tokens = vec![(ENDOFTEXT, 50256usize), (FIM_PREFIX, 50281), (FIM_MIDDLE, 50282), (FIM_SUFFIX, 50283)];
                    const RE: &str = r##"'s|'t|'re|'ve|'m|'ll|'d| ?\p{L}+| ?\p{N}+| ?[^\s\p{L}\p{N}]+|\s+(?!\S)|\s+"##;

                    Arc::new(BpeEncoder::new_internal(typ, data, special_tokens.into_iter(),RE))
                }).clone()
            }
        }
    }

    fn new_internal(
        typ: EncoderType,
        tokens: impl Iterator<Item = (TokenString, TokenInt)>,
        special_tokens: impl Iterator<Item = (&'static str, TokenInt)>,
        regex: &'static str,
    ) -> Self {
        let encoder = hash::TokenEncoder::new(tokens);
        let decoder = encoder.invert();

        let mut sorted_token_bytes = encoder.token_strings().cloned().collect::<Vec<_>>();
        sorted_token_bytes.sort_unstable();

        let special_token_encoder = hash::TokenEncoder::new(special_tokens);
        let special_token_decoder = special_token_encoder.invert();

        Self {
            typ,
            encode: encoder,
            special_tokens_encode: special_token_encoder,
            decode: decoder,
            special_tokens_decode: special_token_decoder,
            regex: fancy_regex::Regex::new(regex).expect("BUG: Invalid regex"),
            sorted_token_bytes,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bpe_encoder() {
        for typ in EncoderType::iter() {
            // TODO: implement gpt2 and then test it.  right now it panics.
            if typ != EncoderType::Gpt2 {
                println!("Loading encoder {typ}");
                let encoder = BpeEncoder::new(typ);
                assert_eq!(encoder.typ, typ);
            } else {
                println!("Skipping encoder {typ} (not implemented yet)");
            }
        }
    }
}
