//! Sit down and get comfortable dear reader, for I have a tale of woe for you.
//!
//! We need to access private parts of the `tiktoken` implementation (which sounds more criminal
//! than it actually is, although if you question the morality of the act I won't blame you).  Not
//! only is `tiktoken` not published on crates.io as of this writing (only forks of dubious
//! provenance which have been modified from the official OpenAI repo), but even if there were a
//! crates.io version we need to access private methods as part of our efforts to unit test our
//! Rust rewrite against the tiktoken reference.
//!
//! Fortunately, the entire Rust impl is in a single file `lib.rs`.  Unfortunately that lib.rs
//! can't be simply `include!` ed into an empty module, because it contains an inner attribute that
//! the compiler doesn't allow in an `include!`d impl.
//!
//! So we have this custom build script that reaches into the tiktoken source (which is available
//! in the `tiktoken` directory by the power of git submodules), modifies the lib.rs source to
//! remove the offending inner attribute line, and writes this modified version to the crate's
//! OUT_DIR directory.  From there we will be able to `include!` it and have our wicked way with
//! it.
//!
//! While I'm not proud of what has been done here today, in my defense it is necessary in order to
//! be able to test against `tiktoken`, and this crate is not published so my sin is hidden within
//! this repo and known only to you, dear reader.  I hope I can rely on your discretion.
use std::io::Write;
use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error + Sync + Send + 'static>> {
    const INPUT_TIKTOKEN_SRC: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tiktoken/src/lib.rs");

    let input_tiktoken_src = std::fs::read_to_string(INPUT_TIKTOKEN_SRC).unwrap_or_else(|e| {
        panic!(
            "Failed to open tiktoken source file `{INPUT_TIKTOKEN_SRC}`.  \
            Did you forget to run `git submodule update --init --recursive` after checking out this repo? \
            \
            Error details: {e}"
        );
    });

    let output_toktoken_src =
        Path::new(&std::env::var("OUT_DIR").unwrap()).join("tiktoken-hacked.rs");
    let mut output_file = std::fs::File::create(output_toktoken_src)?;

    for line in input_tiktoken_src.lines() {
        if line.starts_with("#![") {
            // Skip this line
        } else {
            writeln!(&mut output_file, "{}", line)?;
        }
    }

    println!("cargo:rerun-if-changed={}", INPUT_TIKTOKEN_SRC);

    Ok(())
}
