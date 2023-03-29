//! Contains the implementation of the bulk of the tokenization logic, implemented as a Rust
//! [`Iterator`].
use crate::{bpe, BpeEncoderParams, TokenInt};
use std::collections::VecDeque;
use std::sync::Arc;

/// The internal state of all iterator variants.  Different iterators work differently and yield
/// different results, but this is the beating heart of them all
#[derive(Clone)]
pub(crate) struct IteratorState<'a> {
    /// The encoder that defines the parameters for the encoding this iterator applies
    params: Arc<BpeEncoderParams>,

    /// The input text that is being tokenized and encoded
    text: &'a str,
}

impl<'a> IteratorState<'a> {
    pub(crate) fn new(params: Arc<BpeEncoderParams>, text: &'a str) -> Self {
        Self { params, text }
    }
}

/// Applies the encoding's regex to get the next "word", as defined by the regex itself.
///
/// Updates `pos` accordingly
struct WordIterator<'a> {
    state: IteratorState<'a>,

    /// The offset into `bytes` where the next iteration should start to process
    pos: usize,
}

impl<'a> WordIterator<'a> {
    fn new(state: IteratorState<'a>) -> Self {
        Self { state, pos: 0 }
    }
}

impl<'a> Iterator for WordIterator<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<Self::Item> {
        self.state
            .params
            .regex
            .find_from_pos(self.state.text, self.pos)
            .expect("BUG: regex is not valid")
            .map(|m| {
                self.pos = m.end();

                &self.state.text[m.start()..m.end()]
            })
    }
}

/// The iterator which yields the tokens from a string of text or bytes encoded without regard for
/// special tokens.
///
/// See [`crate::Encoding::encode_ordinary`]
pub struct EncodeOrdinaryIterator<'a> {
    state: IteratorState<'a>,
    words: WordIterator<'a>,

    /// Tokens found in the current word, if the word itself didn't map directly to a token.
    /// If there are any tokens here, they're removed from the back of the queue one iteration
    /// cycle at a time
    current_word_tokens: VecDeque<TokenInt>,
}

impl<'a> EncodeOrdinaryIterator<'a> {
    pub(crate) fn new(state: IteratorState<'a>) -> Self {
        Self {
            words: WordIterator::new(state.clone()),
            state,
            current_word_tokens: VecDeque::new(),
        }
    }

    /// Augment this iterator of tokens, to include the byte offset in the input text where the
    /// token appears, and the byte string contents of that token as it appears in the input text.
    pub fn with_details(self) -> TokenDetailsIterator<'a, Self> {
        TokenDetailsIterator::new(self.state.clone(), self)
    }
}

impl<'a> Iterator for EncodeOrdinaryIterator<'a> {
    type Item = TokenInt;

    fn next(&mut self) -> Option<Self::Item> {
        // If there are still tokens left from a previously iterated word, use one of them
        if let Some(token) = self.current_word_tokens.pop_back() {
            Some(token)
        } else if let Some(word) = self.words.next() {
            // This is the next word in the input.  Either it is itself a token, in which case
            // we're done, or it's not and we need to break the word up into subword tokens with
            // the BPE algorithm
            if let Some(token) = self.state.params.encode.token_for_bytes(word) {
                // Easy, the word is a token
                Some(token)
            } else {
                // The word is not a token, so we need to break it up into subword tokens
                // There's no value in building an iterator type just to handle this case.  Do the
                // iterative merging for the whole word
                let tokens = bpe::byte_pair_encode(word.as_bytes(), &self.state.params.encode);
                debug_assert!(!tokens.is_empty());

                // TODO OPTIMIZATION: re-use the same dequeue, and modify byte_pair_encode to take
                // the dequeue as an arg instead of allocating a new vec each time.

                // Push the tokens into the dequeue at the front.  They are popped out the back.
                self.current_word_tokens.clear();

                let mut tokens = tokens.into_iter();
                let first_token = tokens
                    .next()
                    .expect("BUG: encoding always yields at least one token");
                for token in tokens {
                    self.current_word_tokens.push_front(token)
                }

                Some(first_token)
            }
        } else {
            // End of the line.  No more words to process, and no tokens left over from a
            // previously processed word.
            None
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        // Use the average token length to estimate how many tokens will be in this iterator.
        // It's not precise by it's better than nothing for pre-allocating vectors and such.
        (self.state.params.estimate_num_tokens(self.state.text), None)
    }
}

/// An iterator which wraps one of the other iterators, and yields not only the integer token
/// [`TokenInt`] but also the byte string that the token came from, and the position of the token
/// in the input text
///
/// To get this additional detail from an iterator of tokens, see the `with_details` method on a
/// token iterator.
///
/// The tuple this iterator yields is of the form `(token_int, byte_offset, byte_string)`
///
/// Note that this iterator adds some additional overhead to retrieve the byte strings for each
/// token, so don't use it unless you need this additional context.
pub struct TokenDetailsIterator<'a, I> {
    state: IteratorState<'a>,
    tokens: I,
    pos: usize,
}

impl<'a, I: Iterator<Item = TokenInt>> TokenDetailsIterator<'a, I> {
    pub(crate) fn new(state: IteratorState<'a>, tokens: I) -> Self {
        Self {
            state,
            tokens,
            pos: 0,
        }
    }
}

impl<'a, I: Iterator<Item = TokenInt>> Iterator for TokenDetailsIterator<'a, I> {
    type Item = (TokenInt, usize, &'a [u8]);

    fn next(&mut self) -> Option<Self::Item> {
        self.tokens.next().map(|token| {
            // How long is this token's byte string?
            let token_len = self
                .state
                .params
                .decode
                .bytes_for_token(token)
                .expect("BUG: token not found in decoding")
                .len();

            // Unfortunately there is no guarantee that tokens fall exactly on UTF-8 code points,
            // so we can't assume individual tokens are strings, even though we know the original
            // input string is.
            let token_bytes = &self.state.text.as_bytes()[self.pos..self.pos + token_len];
            let offset = self.pos;
            self.pos += token_len;

            (token, offset, token_bytes)
        })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.tokens.size_hint()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EncodingType;
    use proptest::prelude::*;

    #[test]
    fn word_iterator_works() {
        fn get_words(params: Arc<BpeEncoderParams>, text: &str) -> Vec<&str> {
            let state = IteratorState::new(params, text);
            let words = WordIterator::new(state);

            words.collect()
        }

        let params = crate::BpeEncoderParams::load(EncodingType::Cl100kBase);

        assert!(get_words(params.clone(), "").is_empty());
        assert_eq!(&["foo"], get_words(params.clone(), "foo").as_slice());
        assert_eq!(
            &["foo", " bar", " baz"],
            get_words(params.clone(), "foo bar baz").as_slice()
        );
        assert_eq!(
            &[
                "let",
                " params",
                " =",
                " crate",
                "::",
                "BpeEncoderParams",
                "::",
                "load",
                "(EncodingType",
                "::",
                "Cl",
                "100",
                "kBase",
                ");"
            ],
            get_words(
                params,
                r##"let params = crate::BpeEncoderParams::load(EncodingType::Cl100kBase);"##
            )
            .as_slice()
        );
    }

    #[test]
    fn ordinary_tokens_iterator_works() {
        fn get_detailed_tokens(
            params: Arc<BpeEncoderParams>,
            text: &str,
        ) -> Vec<(TokenInt, usize, &str)> {
            let tokens = EncodeOrdinaryIterator::new(IteratorState::new(params, text));
            let tokens = tokens.with_details();

            // At the API level we can't assume tokens are always UTF-8 strings, however inside
            // this test we only test tokens that are, so we can assume this and make the tests
            // more readable
            let tokens = tokens.map(|(token, offset, byte_string)| {
                (
                    token,
                    offset,
                    std::str::from_utf8(byte_string).expect("BUG: expected UTF-8 token"),
                )
            });
            tokens.collect()
        }

        let params = crate::BpeEncoderParams::load(EncodingType::Cl100kBase);

        assert!(get_detailed_tokens(params.clone(), "").is_empty());
        assert_eq!(
            &[(8134, 0, "foo")],
            get_detailed_tokens(params.clone(), "foo").as_slice()
        );
        assert_eq!(
            &[
                (69, 0, "f"),
                (686, 1, "ear"),
                (374, 4, " is"),
                (279, 7, " the"),
                (4059, 11, " mind"),
                (50965, 16, " kk"),
                (11088, 19, "kill"),
                (1565, 23, "ler")
            ],
            get_detailed_tokens(params, "fear is the mind kkkilller").as_slice()
        );
    }

    proptest! {
        /// Run the encoding against many example strings, testing that the detailed iterator
        /// yields values that can be used to reconstruct the original string.
        ///
        /// This isn't verifying that the tokenization is correct; that's done in the `bench`
        /// crate and uses `tiktoken` as the reference.  It's verifying that the iterator works
        /// right, assuming the tokenization si correct.
        #[test]
        fn detailed_iterator_correct(s in "\\PC*") {
            let encoding = crate::Encoding::new(EncodingType::Cl100kBase);

            let tokens = encoding.encode_ordinary(&s);
            let tokens = tokens.with_details().collect::<Vec<_>>();

            // Sanity check the offsets from the details, and re-construct the original string with
            // the given byte strings
            let mut original_text = Vec::with_capacity(s.len());

            let mut next_offset = 0;
            for (_token, offset, byte_string) in tokens {
                assert_eq!(offset, next_offset);
                assert_eq!(byte_string, &s.as_bytes()[offset..offset+byte_string.len()]);
                original_text.extend_from_slice(byte_string);
                next_offset = offset + byte_string.len();
            }

            let original_text = String::from_utf8(original_text).expect("BUG: expected UTF-8 string");

            assert_eq!(s, original_text);
        }
    }
}
