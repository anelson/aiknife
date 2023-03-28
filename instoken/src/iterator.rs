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

#[cfg(test)]
mod tests {
    use crate::EncodingType;

    use super::*;

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
}
