//! Tests to compare our byte pair merge function against `tiktoken`s for correctness only.
use instoken::{BpeEncoderParams, EncodingType};
use instoken_bench::tiktoken;
use proptest::prelude::*;

/// Simple sanity check matches with toktoken.  Will use proptest for the exhaustive testing
#[test]
fn byte_pair_encode_simple() {
    let params = crate::BpeEncoderParams::load(EncodingType::Cl100kBase);

    const TEXT: &str = "foo bar baz";

    let our_tokens = instoken::byte_pair_encode(TEXT.as_bytes(), &params.encode);

    let vocab = params
        .encode
        .tokens()
        .map(|(bytes, int)| (bytes.clone(), int))
        .collect();

    let their_tokens = tiktoken::byte_pair_encode(TEXT.as_bytes(), &vocab);

    assert_eq!(our_tokens, their_tokens);
}

proptest! {
    /// encoding any printable characters should produce the same results with our impl or
    /// tiktoken's
    ///
    /// tiktoken's `byte_pair_encode` panics when passed an empty slice, so we'll limit ourselves
    /// to one or more characters only
    #[test]
    fn byte_pair_encodes_anything(s in "\\PC+") {
        let params = crate::BpeEncoderParams::load(EncodingType::Cl100kBase);
        let vocab = params
            .encode
            .tokens()
            .map(|(bytes, int)| (bytes.clone(), int))
            .collect();
        let our_tokens = instoken::byte_pair_encode(s.as_bytes(), &params.encode);
        let their_tokens = tiktoken::byte_pair_encode(s.as_bytes(), &vocab);
        assert_eq!(our_tokens, their_tokens);

        let our_tokens = instoken::byte_pair_split(s.as_bytes(), &params.encode);
        let their_tokens = tiktoken::byte_pair_split(s.as_bytes(), &vocab);
        assert_eq!(our_tokens, their_tokens);
    }
}
