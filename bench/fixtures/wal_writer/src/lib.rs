//! Append-only write-ahead log with checksums and torn-tail recovery.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::Path;

/// Record layout on disk (all integers little-endian):
/// [4 bytes: FNV-1a 32-bit checksum of payload]
/// [4 bytes: payload length (u32)]
/// [payload bytes]
///
/// A torn (partially written) final record — checksum mismatch or
/// truncated header/payload — is discarded on recovery.

pub struct WalWriter {
    file: File,
}

impl WalWriter {
    /// Open (or create) the log for appending.
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        todo!("Step 1")
    }

    /// Append one record; returns its 1-based sequence number.
    pub fn append(&mut self, payload: &[u8]) -> io::Result<u32> {
        todo!("Step 1")
    }
}

/// Recover all intact records from a log file, truncating any torn
/// tail in place. Returns the payloads in write order.
pub fn recover(path: impl AsRef<Path>) -> io::Result<Vec<Vec<u8>>> {
    todo!("Step 2")
}

/// FNV-1a 32-bit checksum (public for tests and recovery).
pub fn checksum(data: &[u8]) -> u32 {
    todo!("Step 1")
}
