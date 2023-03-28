//! Kids, don't try this at home!
//!
//! If you're 18 or older and not easily horrified, read the comment in `build.rs` that explains
//! the eldritch horrors within this crate.
include!(concat!(env!("OUT_DIR"), "/tiktoken-hacked.rs"));

// Re-export some private details so we can use them in tests
pub use rustc_hash::FxHashMap as TiktokenHashMap;

pub fn byte_pair_merge<T>(
    piece: &[u8],
    ranks: &HashMap<Vec<u8>, usize>,
    f: impl Fn(std::ops::Range<usize>) -> T,
) -> Vec<T> {
    _byte_pair_merge(piece, ranks, f)
}

// Hack to expose the constructor for CoreCPE which is normally only available from Python
pub struct CoreBPEHack(CoreBPE);

impl CoreBPEHack {
    pub fn new(
        encoder: TiktokenHashMap<Vec<u8>, usize>,
        special_tokens_encoder: TiktokenHashMap<String, usize>,
        pattern: &str,
    ) -> PyResult<Self> {
        Ok(Self(CoreBPE::new(
            encoder,
            special_tokens_encoder,
            pattern,
        )?))
    }

    pub fn encode_ordinary(&self, text: &str) -> Vec<usize> {
        self.0._encode_ordinary_native(text)
    }
}
