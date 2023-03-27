/// A token output by the tokenzier, represented in its integer form corresponding to the rank this
/// token has in the encoding's vocabulary.
///
/// Each actual token is just a unique substring from the input text, but in practice the rank
/// of that token is used as an integer representation of that substring for the purposes of
/// feeding into advanced LLMs.  If instead the implementation just returned substrings from the
/// input text, then we'd need another stage that translates those substrings into their integer
/// ranks, and that would just add overhead and slow things down, therefore we take the shortcut to
/// represent tokens by their rank explicitly.
pub type TokenInt = usize;

/// A token produced by the tokenizer, represented in its byte string form.
///
/// Users of tokenizing libraries usually are interested in the integer representation, as defined
/// by [`TokenInt`], but in our internal implementation we need to store token strings as well for
/// lookup purposes.
pub type TokenString = Vec<u8>;
