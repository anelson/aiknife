//! This is a simple test rig that not-so-elegantly pulls in the `tiktoken` implementation from a
//! git submodule, and exposes some of it's nastier inner bits for use with unit testing individual
//! components of the instoken implementation.

pub mod tiktoken;

// Ugly hack to make tiktoken's `lib.rs` compile when included as a separate module
//
// It imports `crate::byte_pair_split` but because we're pulling in that code in a submodule
// `tiktoken`, there won't be a `crate::byte_pair_split` unless we re-export it here
#[allow(unused_imports, dead_code)]
// This is actually used by `tiktoken` above
use tiktoken::byte_pair_split;

// TODO: Start making tests using the tokenizer data from `tokens`.

pub fn add(left: usize, right: usize) -> usize {
    left + right
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_works() {
        let result = add(2, 2);
        assert_eq!(result, 4);
    }
}
