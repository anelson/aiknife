use std::fmt::Debug;
use std::sync::Arc;

mod bpe;
mod encoder;
mod error;
mod iterator;
mod token;

pub use bpe::*;
pub use encoder::*;
pub use error::*;
pub use iterator::*;
pub use token::*;

pub type Result<T> = std::result::Result<T, InstokenError>;

/// An implementation of a specific text encoding used by OpenAI models.
///
/// With this encoding, it's possible to tokenize input text into numeric tokens, corresponding
/// text tokens, or both.  It's also possible to decode integer tokens back into the text they came
/// from.
///
/// Instances of `Encoding` are light weight and can be very cheaply cloned.  They are also thread
/// safe; a single instance can be used to encode or decode text in multiple threads
/// simultaneously, although with Rust ownership rules it's usually more convenient to make a clone
/// of the encoding for each thread.
#[derive(Clone)]
pub struct Encoding {
    params: Arc<BpeEncoderParams>,
}

impl Encoding {
    /// Create a new instance of a text encoding.
    pub fn new(typ: EncodingType) -> Self {
        Self {
            params: BpeEncoderParams::load(typ),
        }
    }
}

impl Encoding {
    /// The special token strings and their corresponding integer ranks, for this encoding
    pub fn special_tokens(&self) -> impl Iterator<Item = (&TokenString, TokenInt)> {
        self.params.special_tokens_encode.tokens()
    }

    /// Search the given input text for "special" tokens, returning an iterator that yields all
    /// special tokens in the input text.
    ///
    /// If you just need to know if there are any special tokens in the text, you can call
    /// [`Iterator::any`]
    pub fn find_special_tokens<'me, 'text>(
        &'me self,
        text: &'text str,
    ) -> impl Iterator<Item = &'text str> + 'text
    where
        'me: 'text,
    {
        self.params
            .special_tokens_finder
            .find_iter(text)
            .map(move |m| &text[m.start()..m.end()])
    }

    /// Make an educated guess as to the number of likely tokens in a given bit of text.
    ///
    /// This is not a precise calculation, but it's a good enough estimate for most purposes.  For
    /// example if you want to pre-allocate a Vec to hold the tokens for a string, this is a good
    /// choice for the target capacity.  It might be slightly off but it's definitely better than no
    /// pre-allocation at all.
    pub fn estimate_num_tokens(&self, text: impl AsRef<str>) -> usize {
        self.params.estimate_num_tokens(text.as_ref())
    }

    /// Encode the specified text into a sequence of tokens, without any special handling for
    /// "special" tokens.  If `text` contains any special tokens, they will not be encoded with the
    /// corresponding `TokenInt` values for special tokens.  Rather, they will be treated like any
    /// other text, and broken up into subword tokens.
    ///
    /// If you don't require detecting special tokens, use the `ordinary` versions, as they are
    /// slightly faster and also simpler to use.
    pub fn encode_ordinary(&self, text: impl AsRef<str>) -> Vec<TokenInt> {
        todo!()
    }

    /// Encode the specified text into a sequence of tokens, including encoding
    /// "special" tokens.  If `text` contains any special tokens, they will be encoded into the
    /// special `TokenInt` values assigned to the special tokens for this encoding.
    ///
    /// If you want to detect text that contains certain special tokens, to avoid abuse of the
    /// model or potentially unwanted behavior, use the [`Self::find_special_tokens`] function to
    /// scan input for special tokens, and take whatever action you need if any are found.
    ///
    /// This is a difference between Instoken and `tiktoken`, which provides a function that will
    /// fail if special tokens are present in the text.  That adds unnecessary complexity to the
    /// implementation, and causes coupling between the encoding logic and input validation that
    /// isn't necessary or desireable.
    ///
    /// If you don't require detecting special tokens, use the `ordinary` versions, as they are
    /// slightly faster and also simpler to use.
    pub fn encode(&self, text: impl AsRef<str>) -> Vec<TokenInt> {
        todo!()
    }

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
    ///
    /// TODO: This is a dumb API design.  Since it doesn't actually return `str`, it's useless for
    /// the intended purpose, which is to split up big texts into small substrings.
    pub fn first_n_tokens(&self, text: &[u8], max_tokens: usize) -> (&[u8], Option<&[u8]>) {
        todo!()
    }

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
    pub fn decode_tokens_to_string(&self, tokens: &[TokenInt]) -> Result<String> {
        let bytes = self.decode_tokens_to_bytes(tokens)?;

        Ok(String::from_utf8(bytes).map_err(|e| todo!())?)
    }

    /// Decode a set of tokens into the bytes that they came from.
    ///
    /// This is fallible only in case the tokens passed in to `tokens` came from a different
    /// encoding, and thus the integer representation doesn't correspond to an actual token byte
    /// sequence.
    pub fn decode_tokens_to_bytes(&self, tokens: &[TokenInt]) -> Result<TokenString> {
        todo!()
    }

    /// Decode a single token into the byte sequence it corresponds to.
    ///
    /// If the token isn't one that is used by this encoding,
    /// this returns `None`.
    fn decode_token(&self, token: TokenInt) -> Option<TokenString> {
        todo!()
    }

    /// Given a token, return the length of the corresponding byte sequence in bytes.
    ///
    /// This works the same as [`Self::decode_token`], except it doesn't incur the heap allocation
    /// to return the bytes themselves.  It can fail with `None` under the same circumstances.
    fn get_token_len(&self, token: TokenInt) -> Option<usize> {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_works() {}
}
