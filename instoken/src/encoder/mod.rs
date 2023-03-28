use crate::{TokenInt, TokenString};
use once_cell::sync::OnceCell;
use std::sync::Arc;
use strum::{EnumIter, EnumString, EnumVariantNames, IntoEnumIterator};

mod data;
mod hash;

pub(crate) use hash::{TokenDecoder, TokenEncoder};

/// The tokenizer encoding to use to tokenize text.
///
/// Each of these uses a BPE subword tokenizing approach, but with different sets of
/// tokens and corresponding ranks, and with a different regex for breaking up text into
/// approximate word boundaries.
#[derive(
    Clone, Copy, Debug, Eq, PartialEq, EnumString, EnumIter, EnumVariantNames, strum::Display,
)]
pub enum EncodingType {
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
pub struct BpeEncoderParams {
    pub(crate) typ: EncodingType,

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

    /// A/C automaton for finding special tokens in text
    pub(crate) special_tokens_finder: aho_corasick::AhoCorasick,

    /// All token strings, sorted lexicographically
    pub(crate) sorted_token_bytes: Vec<TokenString>,

    /// The mean length of a token in bytes.
    ///
    /// This is used when we need to estimate how many tokens are likely to be in a string.
    pub(crate) mean_token_len: usize,
}

impl BpeEncoderParams {
    /// Load the parameters for the given encoder type, if not already present.
    ///
    /// Each encoder type is lazily loaded on demand, but only once.  After that the loaded encoder
    /// is held in memory for the duration of the process.  The resulting `Arc` is very cheap to
    /// clone.
    pub fn load(typ: EncodingType) -> Arc<Self> {
        // The following was adapted from the python `openai_public.py` from the `tiktokens` source
        const ENDOFTEXT: &str = "<|endoftext|>";
        const FIM_PREFIX: &str = "<|fim_prefix|>";
        const FIM_MIDDLE: &str = "<|fim_middle|>";
        const FIM_SUFFIX: &str = "<|fim_suffix|>";
        const ENDOFPROMPT: &str = "<|endofprompt|>";

        match typ {
            EncodingType::Cl100kBase => {
                static INSTANCE: OnceCell<Arc<BpeEncoderParams>> = OnceCell::new();
                INSTANCE.get_or_init(|| {
                    let data = data::get_token_data(typ);
                    let special_tokens = vec![(ENDOFTEXT, 100257usize), (FIM_PREFIX, 100258), (FIM_MIDDLE, 100259), (FIM_SUFFIX, 100260), (ENDOFPROMPT, 100276)];
                    const RE: &str = r##"(?i:'s|'t|'re|'ve|'m|'ll|'d)|[^\r\n\p{L}\p{N}]?\p{L}+|\p{N}{1,3}| ?[^\s\p{L}\p{N}]+[\r\n]*|\s*[\r\n]+|\s+(?!\S)|\s+"##;

                    Arc::new(BpeEncoderParams::new(typ, data, special_tokens.into_iter(),RE))
                }).clone()
            }
            EncodingType::Gpt2 => {
                static INSTANCE: OnceCell<Arc<BpeEncoderParams>> = OnceCell::new();
                INSTANCE.get_or_init(|| {
                    let data = data::get_token_data(typ);
                    let special_tokens = vec![(ENDOFTEXT, 50256usize)];
                    const RE: &str = r##"'s|'t|'re|'ve|'m|'ll|'d| ?\p{L}+| ?\p{N}+| ?[^\s\p{L}\p{N}]+|\s+(?!\S)|\s+"##;

                    Arc::new(BpeEncoderParams::new(typ, data, special_tokens.into_iter(),RE))
                }).clone()
            }
            EncodingType::P50kBase => {
                static INSTANCE: OnceCell<Arc<BpeEncoderParams>> = OnceCell::new();
                INSTANCE.get_or_init(|| {
                    let data = data::get_token_data(typ);
                    let special_tokens = vec![(ENDOFTEXT, 50256usize)];
                    const RE: &str = r##"'s|'t|'re|'ve|'m|'ll|'d| ?\p{L}+| ?\p{N}+| ?[^\s\p{L}\p{N}]+|\s+(?!\S)|\s+"##;

                    Arc::new(BpeEncoderParams::new(typ, data, special_tokens.into_iter(),RE))
                }).clone()
            }
            EncodingType::P50kEdit => {
                static INSTANCE: OnceCell<Arc<BpeEncoderParams>> = OnceCell::new();
                INSTANCE.get_or_init(|| {
                    let data = data::get_token_data(typ);
                    let special_tokens = vec![(ENDOFTEXT, 50256usize), (FIM_PREFIX, 50281), (FIM_MIDDLE, 50282), (FIM_SUFFIX, 50283)];
                    const RE: &str = r##"'s|'t|'re|'ve|'m|'ll|'d| ?\p{L}+| ?\p{N}+| ?[^\s\p{L}\p{N}]+|\s+(?!\S)|\s+"##;

                    Arc::new(BpeEncoderParams::new(typ, data, special_tokens.into_iter(),RE))
                }).clone()
            }
            EncodingType::R50kBase => {
                static INSTANCE: OnceCell<Arc<BpeEncoderParams>> = OnceCell::new();
                INSTANCE.get_or_init(|| {
                    let data = data::get_token_data(typ);
                    let special_tokens = vec![(ENDOFTEXT, 50256usize), (FIM_PREFIX, 50281), (FIM_MIDDLE, 50282), (FIM_SUFFIX, 50283)];
                    const RE: &str = r##"'s|'t|'re|'ve|'m|'ll|'d| ?\p{L}+| ?\p{N}+| ?[^\s\p{L}\p{N}]+|\s+(?!\S)|\s+"##;

                    Arc::new(BpeEncoderParams::new(typ, data, special_tokens.into_iter(),RE))
                }).clone()
            }
        }
    }

    /// Make an educated guess as to the number of likely tokens in a given bit of text.
    pub fn estimate_num_tokens(&self, text: &str) -> usize {
        // Use a dumb strategy: we know the mean length of a token in this encoding, and assume the
        // text is full of the mean tokens.
        (text.len() + self.mean_token_len - 1) / self.mean_token_len
    }

    fn new(
        typ: EncodingType,
        tokens: impl Iterator<Item = (TokenString, TokenInt)>,
        special_tokens: impl Iterator<Item = (&'static str, TokenInt)>,
        regex: &'static str,
    ) -> Self {
        let encoder = hash::TokenEncoder::new(tokens);
        let decoder = encoder.invert();

        let mut sorted_token_bytes = encoder.token_strings().cloned().collect::<Vec<_>>();
        sorted_token_bytes.sort_unstable();

        // Calculate the average length of the tokens
        let mean_token_len = sorted_token_bytes
            .iter()
            .map(|bytes| bytes.len() as u64)
            .sum::<u64>()
            / sorted_token_bytes.len() as u64;

        let special_token_encoder = hash::TokenEncoder::new(special_tokens);
        let special_token_decoder = special_token_encoder.invert();

        // Make an aho-corasick automaton that quickly finds any of the special tokens in text.
        // This is faster than a regex according to my benchmark tests
        let special_tokens = special_token_encoder.token_strings();
        let special_tokens_finder = aho_corasick::AhoCorasickBuilder::new()
            .dfa(true) // This benchmarks slightly faster than the default NFA
            .build(special_tokens);

        Self {
            typ,
            encode: encoder,
            special_tokens_encode: special_token_encoder,
            decode: decoder,
            special_tokens_decode: special_token_decoder,
            regex: fancy_regex::Regex::new(regex).expect("BUG: Invalid regex"),
            special_tokens_finder,
            sorted_token_bytes,
            mean_token_len: mean_token_len as usize,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bpe_encoder() {
        for typ in EncodingType::iter() {
            // TODO: implement gpt2 and then test it.  right now it panics.
            if typ != EncodingType::Gpt2 {
                println!("Loading encoder {typ}");
                let encoder = BpeEncoderParams::load(typ);
                assert_eq!(encoder.typ, typ);
            } else {
                println!("Skipping encoder {typ} (not implemented yet)");
            }
        }
    }
}
