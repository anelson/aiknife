//! Implementation of components of BPE (byte pair encoding) as it relates to encoding text as
//! tokens.
use crate::encoder::TokenEncoder;
use crate::TokenInt;

/// Using the BPE algorithm find the tokens in a word and return their integer form.
pub fn byte_pair_encode(word: &[u8], encoder: &TokenEncoder) -> Vec<TokenInt> {
    // It's assumed that all 256 possible single byte values are also tokens
    if word.len() == 1 {
        return vec![encoder
            .token_for_bytes(word)
            .expect("BUG: expect all possible u8 values to correspond to a token")];
    }

    byte_pair_merge(word, encoder, |p| {
        encoder
            .token_for_bytes(&word[p.start..p.end])
            .expect("BUG: expect p to already be matched to a token")
    })
}

/// Using the BPE algorithm, find the tokens in a word and return their byte string form.
/// TODO: why is this useful?
pub fn byte_pair_split<'a>(word: &'a [u8], encoder: &TokenEncoder) -> Vec<&'a [u8]> {
    if word.len() == 1 {
        return vec![word];
    }
    byte_pair_merge(word, encoder, |p| &word[p.start..p.end])
}

/// Merge bytes within a word together into progressively fewer, longer byte sequences that
/// correspond to tokens in the encoding vocabulary.
///
/// The token integer representation is assumed to also be the "rank" of that token.  That's
/// important since the algorithm works by taking the token with the minimum rank to merge into an
/// adjacent token, iteratively, until all that's left is the tokens that can't be merged with
/// their adjacent tokens to produce more tokens.
///
/// `f` is a function that, when called on a range that is assumed to be inside of `word`, returns
/// either the integer or string form of the token in that range, depending on `T`.  That's a dumb
/// way to do it and I need to refactor that.
///
/// Based on the _byte_pair_merge function in the `tiktoken` source.
fn byte_pair_merge<T>(
    word: &[u8],
    encoder: &TokenEncoder,
    f: impl Fn(std::ops::Range<usize>) -> T,
) -> Vec<T> {
    // This is a vector of (start, rank).
    // The rank is of the byte pair starting at position start.
    // The rank of the last item in the vector is not a valid value.
    let mut parts: Vec<(usize, usize)> = (0..word.len() + 1).map(|i| (i, usize::MAX)).collect();

    // The token for a range of bytes is assumed to also be its merge rank
    let get_rank = {
        #[inline(always)]
        |parts: &Vec<(usize, usize)>, start_idx: usize, skip: usize| {
            if (start_idx + skip + 2) < parts.len() {
                encoder.token_for_bytes(&word[parts[start_idx].0..parts[start_idx + skip + 2].0])
            } else {
                None
            }
        }
    };

    // We look up the ranks once in the beginning and iteratively update
    // them during each merge, which reduces the number of rank lookups.
    for i in 0..parts.len() - 2 {
        match get_rank(&parts, i, 0) {
            Some(rank) => {
                // usize::MAX is a sentinel value and cannot be a valid rank
                debug_assert!(rank != usize::MAX);
                parts[i].1 = rank;
            }
            None => {
                continue;
            }
        };
    }

    // If you have n parts and m merges, this does O(mn) work.
    // We could do something with a heap and do O(m log n) work.
    // It is important to consider that n is often small (<100), and as such
    // the cache-locality benefits outweigh the algorithmic complexity downsides
    // of the `parts` vector data structure above.

    // Note that we hash bytes, not token pairs. As long as we train BPE the way we
    // currently do, this is equivalent. An easy way to break this would be to decouple
    // merge priority from token index or to prevent specific token merges.
    loop {
        if parts.len() == 1 {
            break;
        }

        // usize::MAX is a sentinel rank value allowing us to
        // take the min more quickly
        let mut min_rank: (usize, usize) = (usize::MAX, 0);
        for (i, &(_, rank)) in parts[..parts.len() - 1].iter().enumerate() {
            if rank < min_rank.0 {
                min_rank = (rank, i);
            }
        }

        if min_rank.0 != usize::MAX {
            let i = min_rank.1;

            // NOTE: We are about to remove parts[i + 1]. We do not do it
            // yet because there are cache-locality benefits to updating
            // parts[i] and parts[i-1] before removing, which could thrash
            // the cache. Thus, we update the rank calculation by skipping over
            // parts[i + 1], by invoking `get_rank!` with `skip = 1`.
            parts[i].1 = get_rank(&parts, i, 1).unwrap_or(usize::MAX);
            if i > 0 {
                parts[i - 1].1 = get_rank(&parts, i - 1, 1).unwrap_or(usize::MAX);
            }

            parts.remove(i + 1);
        } else {
            break;
        }
    }
    let mut out: Vec<T> = Vec::with_capacity(parts.len() - 1);
    for i in 0..parts.len() - 1 {
        out.push(f(parts[i].0..parts[i + 1].0));
    }
    out
}
