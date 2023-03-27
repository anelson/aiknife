use std::fmt::Debug;
use strum::{EnumIter, EnumString, EnumVariantNames, IntoEnumIterator};

mod encoder;
mod error;
mod token;

pub use encoder::*;
pub use error::*;
pub use token::*;

pub type Result<T> = std::result::Result<T, InstokenError>;

/// A tokenizer implementation that tokenizes text using some specified encoding.
pub trait Tokenizer: Clone + Debug {
    /// Encode the specified text into a sequence of tokens
    fn encode_string(&self, text: &str) -> Vec<TokenInt> {
        self.encode_bytes(text.as_bytes())
    }

    /// Encode the specified bytes into a sequence of token IDs.
    ///
    /// While tokenizers are intended to be run against text, and not binary data, we can't always
    /// assume that the input text is clean UTF-8, which is what Rust uses internally for the
    /// `&str` type.  Therefore encoding bytes is also possible.  The underlying algorithms and
    /// performance is identical.  If you're working with text, prefer [`Self::encode_string`].
    fn encode_bytes(&self, bytes: &[u8]) -> Vec<TokenInt>;

    /// Split the input bytes into two parts: the first `max_tokens` tokens, and all of the rest.
    ///
    /// Note that this is only available for byte slices and not strings, because there is no
    /// guarantee that the tokenizer will split a string's bytes in such a way as to guarantee that
    /// both sides of the max_tokens boundary are valid UTF-8 strings.
    ///
    /// Use this when you don't actually care about the tokens, but you need to keep the input text
    /// under an API-defined token limit.
    ///
    /// If you need this to product a string type, you can use `String::from_utf8_lossy` but that
    /// will incur a heap allocation.
    fn first_n_tokens(&self, text: &[u8], max_tokens: usize) -> (&[u8], Option<&[u8]>);

    /// Decode a set of tokens into the string that they came from.
    ///
    /// Note that encoders can sometimes take a valid UTF-8 string as input, and break it down into
    /// subword tokens such that some of those tokens are not actually valid UTF-8 code points.
    /// Therefore, decoding to a string is a fallible operation.  As long as the input `tokens`
    /// came from a UTF-8 string, and the sequence of tokens has not been modified or truncated,
    /// then this will succeed.
    ///
    /// If you need more robust decoding, use [`Self::decode_tokens_to_bytes`] which will always
    /// succeed unless you pass tokens that are not used by the encoding which is performding the
    /// decoding.
    fn decode_tokens_to_string(&self, tokens: &[TokenInt]) -> Result<String> {
        let bytes = self.decode_tokens_to_bytes(tokens)?;

        Ok(String::from_utf8(bytes).map_err(|e| todo!())?)
    }

    /// Decode a set of tokens into the bytes that they came from.
    ///
    /// This is fallible only in case the tokens passed in to `tokens` came from a different
    /// encoding than this tokenizer instance uses, and thus the integer representation doesn't
    /// correspond to an actual token byte sequence.
    fn decode_tokens_to_bytes(&self, tokens: &[TokenInt]) -> Result<TokenString>;

    /// Decode a single token into the byte sequence it corresponds to.
    ///
    /// If the token isn't one that is used by the encoder associated with this tokenizer, then
    /// this returns `None`.
    fn decode_token(&self, token: TokenInt) -> Option<TokenString>;

    /// Given a token, return the length of the corresponding byte sequence in bytes.
    ///
    /// This works the same as [`Self::decode_token`], except it doesn't incur the heap allocation
    /// to return the bytes themselves.  It can fail with `None` under the same circumstances.
    fn get_token_len(&self, token: TokenInt) -> Option<usize>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_works() {}
}
