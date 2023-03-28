//! Compare the encoding of full text strings into tokens, with the tiktoken ref impl
use instoken::EncodingType;
use instoken_bench::tiktoken;
use proptest::prelude::*;

fn make_tiktoken_corebpe(encoding: &instoken::Encoding) -> tiktoken::CoreBPEHack {
    let encoder = encoding
        .params()
        .encode
        .tokens()
        .map(|(bytes, int)| (bytes.clone(), int))
        .collect();
    let special_tokens_encoder = encoding
        .params()
        .special_tokens_encode
        .tokens()
        .map(|(bytes, int)| (String::from_utf8(bytes.clone()).unwrap(), int))
        .collect();
    let regex = encoding.params().regex.as_str();

    tiktoken::CoreBPEHack::new(encoder, special_tokens_encoder, regex).unwrap()
}

proptest! {
    #[test]
    fn encode_ordinary_matches(s in "\\PC*") {
        let encoding = instoken::Encoding::new(EncodingType::Cl100kBase);
        let tiktoken_encoding = make_tiktoken_corebpe(&encoding);

        let our_tokens = encoding.encode_ordinary(&s).collect::<Vec<_>>();
        let their_tokens = tiktoken_encoding.encode_ordinary(&s);

        assert_eq!(our_tokens, their_tokens);
    }
}
