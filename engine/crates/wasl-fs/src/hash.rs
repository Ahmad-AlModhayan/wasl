//! Content hashes: MD5 (what Drive reports) and BLAKE3 (what Wasl uses between devices), in one
//! pass over the file.

use md5::{Digest, Md5};
use std::io::{self, Read};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentHash {
    /// Lowercase hex, comparable with Drive's `md5Checksum`.
    pub md5: String,
    /// Lowercase hex.
    pub blake3: String,
    pub size: u64,
}

/// Hashes everything `r` yields.
pub fn hash_reader(mut r: impl Read) -> io::Result<ContentHash> {
    let mut md5 = Md5::new();
    let mut b3 = blake3::Hasher::new();
    let mut buf = vec![0u8; 256 * 1024];
    let mut size = 0u64;
    loop {
        let n = match r.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        md5.update(&buf[..n]);
        b3.update(&buf[..n]);
        size += n as u64;
    }
    Ok(ContentHash {
        md5: hex(&md5.finalize()),
        blake3: b3.finalize().to_hex().to_string(),
        size,
    })
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}
