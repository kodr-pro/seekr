//! RFC 4648 base32 (A-Z, 2-7) with `=` padding.

const ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// Encode bytes as RFC 4648 base32 with padding to a multiple of 8.
pub fn encode(data: &[u8]) -> String {
    todo!("Step 1")
}

/// Decode base32 (case-insensitive, padding required for incomplete
/// groups). Returns Err for invalid characters or wrong padding.
pub fn decode(text: &str) -> Result<Vec<u8>, String> {
    todo!("Step 2")
}
