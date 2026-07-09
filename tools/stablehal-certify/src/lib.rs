//! Library surface of `stablehal-certify`, shared by the CLI binary and its
//! integration tests: the envelope JSON format and the containment check.
//! See `main.rs` for the CLI itself.

pub mod contain;
pub mod envelope;

pub fn arg_value(args: &[String], key: &str) -> Option<String> {
    args.iter()
        .position(|a| a == key)
        .and_then(|i| args.get(i + 1))
        .cloned()
}
