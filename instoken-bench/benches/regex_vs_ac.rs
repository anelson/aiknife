//! `tiktoken` uses `fancy_regex` to search the text for known special tokens.  This is incredibly
//! wasteful, since `fancy_regex` is carrying a ton of baggage to support, well, "fancy" regexs,
//! but special tokens are always specified as string literals.
//!
//! This benchmark compares the cost of searching some large text for a set of special tokens with
//! both impls.

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};

#[inline]
fn fibonacci(n: u64) -> u64 {
    match n {
        0 => 1,
        1 => 1,
        n => fibonacci(n - 1) + fibonacci(n - 2),
    }
}

pub fn criterion_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("Regex vs Aho-Corasick");

    const SPECIAL_TOKENS: &[&str] = &[
        "<|endoftext|>",
        "<|fim_prefix|>",
        "<|fim_middle|>",
        "<|fim_suffix|>",
        "<|endofprompt|>",
    ];

    // Generate some test texts that we will search
    let test_cases = vec![
        " ".to_string(),
        "hello world".to_string(),
        "this is a slightly longer string but it still doesn't have any occurrences of the specail token".to_string(),
        "це ще довший рядок, але в ньому також немає токена.".to_string(),
        "finally a string that has the tokens in it! <|fim_suffix|>".to_string(),
        "this is a very long string holy shit it's still going OMG make it stop".repeat(1000),
        format!("{}<|endoftext|>", "after all of this text there will be on instance of a special token at the very fucking end".repeat(1000))
    ];

    for i in 0..test_cases.len() {
        group.bench_with_input(BenchmarkId::new("regex", i), &i, |b, i| {
            // Make two versions.  One is a regex that uses | to match for any of the special tokens
            let special_regex = {
                let parts = SPECIAL_TOKENS
                    .iter()
                    .map(|s| fancy_regex::escape(s))
                    .collect::<Vec<_>>();
                fancy_regex::Regex::new(&parts.join("|")).unwrap()
            };

            b.iter(|| {
                let mut next_special;
                let mut start_find = 0;
                loop {
                    // Find the next allowed special token, if any
                    next_special = special_regex
                        .find_from_pos(&test_cases[*i], start_find)
                        .unwrap();
                    match next_special {
                        Some(m) => {
                            start_find = m.start() + 1;
                        }
                        None => break,
                    }
                }
            })
        });
        group.bench_with_input(BenchmarkId::new("aho-corasick NFA", i), &i, |b, i| {
            // A/C using the default NFA
            let ac_nfa = aho_corasick::AhoCorasickBuilder::new().build(SPECIAL_TOKENS.iter());

            b.iter(|| {
                let mut text = test_cases[*i].as_str();

                loop {
                    match ac_nfa.find(text) {
                        Some(m) => {
                            // Found this instance; keep searching after
                            text = &text[m.end()..];
                        }
                        None => break,
                    }
                }
            })
        });
        group.bench_with_input(BenchmarkId::new("aho-corasick DFA", i), &i, |b, i| {
            // A/C with the DFA seems like it is perfect for this use case with just a few small strings to
            // match
            let ac_dfa = aho_corasick::AhoCorasickBuilder::new()
                .dfa(true)
                .build(SPECIAL_TOKENS.iter());

            b.iter(|| {
                let mut text = test_cases[*i].as_str();

                loop {
                    match ac_dfa.find(text) {
                        Some(m) => {
                            // Found this instance; keep searching after
                            text = &text[m.end()..];
                        }
                        None => break,
                    }
                }
            })
        });
    }
    group.finish();
}
criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
