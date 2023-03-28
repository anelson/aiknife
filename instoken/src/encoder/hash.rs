//! The choice of hash algorithm and hash function used to maintain the lookup table mapping string
//! inputs to token rank numbers has a big impact on performance.
//!
//! This is isolated to this module to make it easier to experiment with different impls.
use crate::token::{TokenInt, TokenString};

pub use rustc_hash::FxHashMap as HashMap;

/// Encoders maintain the mapping between byte sequences and the ranks assigned to them in the
/// vocabulary.
#[derive(Clone, Debug)]
pub struct TokenEncoder(HashMap<TokenString, TokenInt>);

/// Decoders maintain the reverse mapping, from the integer representation of the token to the
/// corresponding byte sequence.
#[derive(Clone, Debug)]
pub struct TokenDecoder(HashMap<TokenInt, TokenString>);

// Any iterator of token string/ token int pairs can be used to construct either an encoder or
// decoder

impl TokenEncoder {
    pub fn new<Iter, Bytes, Int>(items: Iter) -> Self
    where
        Iter: IntoIterator<Item = (Bytes, Int)>,
        Bytes: Into<TokenString>,
        Int: Into<TokenInt>,
    {
        Self(
            items
                .into_iter()
                .map(|(bytes, rank)| (bytes.into(), rank.into()))
                .collect(),
        )
    }

    pub fn token_for_bytes(&self, bytes: impl AsRef<[u8]>) -> Option<TokenInt> {
        self.0.get(bytes.as_ref()).copied()
    }

    /// Invert the lookup table so the keys become the values, which is another way of describing a
    /// decoder
    pub fn invert(&self) -> TokenDecoder {
        TokenDecoder::new(self.0.clone())
    }

    /// All of the string/int pairs corresponding to special tokens
    pub fn tokens(&self) -> impl Iterator<Item = (&TokenString, TokenInt)> {
        self.0.iter().map(|(s, i)| (s, *i))
    }

    /// All token strings in the encoder
    pub fn token_strings(&self) -> impl Iterator<Item = &TokenString> {
        self.0.keys()
    }
}

impl TokenDecoder {
    pub fn new<Iter, Bytes, Int>(items: Iter) -> Self
    where
        Iter: IntoIterator<Item = (Bytes, Int)>,
        Bytes: Into<TokenString>,
        Int: Into<TokenInt>,
    {
        Self(
            items
                .into_iter()
                .map(|(bytes, rank)| (rank.into(), bytes.into()))
                .collect(),
        )
    }

    pub fn bytes_for_token(&self, token: TokenInt) -> Option<&TokenString> {
        self.0.get(&token)
    }

    /// Without actually decoding this token int, return the length of the corresponding byte
    /// sequence.
    pub fn token_len(&self, token: TokenInt) -> Option<usize> {
        self.bytes_for_token(token).map(TokenString::len)
    }
}
