//! The identity blob a placeholder carries: Cloud Files hands it back with every callback, so a
//! fetch knows which [`FileRef`] to read without a lookup. At most 4 KiB (the API's limit).
//!
//! ```text
//! file:   1 || size (u64 LE) || id length (u16 LE) || id || version length (u16 LE) || version
//! folder: 2
//! ```

use wasl_vfs::FileRef;

/// The API's limit on an identity blob.
pub const MAX: usize = 4096;

const FILE: u8 = 1;
const DIR: u8 = 2;

/// What a placeholder's identity says it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Identity {
    File(FileRef),
    Dir,
}

/// The blob for a file; `None` when its id and version do not fit.
pub fn encode_file(f: &FileRef) -> Option<Vec<u8>> {
    let (id, version) = (f.id.as_bytes(), f.version.as_bytes());
    let len = 1 + 8 + 2 + id.len() + 2 + version.len();
    if len > MAX {
        return None;
    }
    let mut out = Vec::with_capacity(len);
    out.push(FILE);
    out.extend_from_slice(&f.size.to_le_bytes());
    out.extend_from_slice(&(id.len() as u16).to_le_bytes());
    out.extend_from_slice(id);
    out.extend_from_slice(&(version.len() as u16).to_le_bytes());
    out.extend_from_slice(version);
    Some(out)
}

/// The blob for a folder.
pub fn encode_dir() -> Vec<u8> {
    vec![DIR]
}

/// Reads a blob back; `None` when it is not one of ours.
pub fn decode(b: &[u8]) -> Option<Identity> {
    match *b.first()? {
        DIR if b.len() == 1 => Some(Identity::Dir),
        FILE => {
            let size = u64::from_le_bytes(b.get(1..9)?.try_into().ok()?);
            let (id, rest) = take_str(b.get(9..)?)?;
            let (version, rest) = take_str(rest)?;
            rest.is_empty().then(|| {
                Identity::File(FileRef {
                    id: id.to_owned(),
                    version: version.to_owned(),
                    size,
                })
            })
        }
        _ => None,
    }
}

fn take_str(b: &[u8]) -> Option<(&str, &[u8])> {
    let len = u16::from_le_bytes(b.get(..2)?.try_into().ok()?) as usize;
    let s = std::str::from_utf8(b.get(2..2 + len)?).ok()?;
    Some((s, &b[2 + len..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_rejects_garbage() {
        let f = FileRef {
            id: "1AbC-drive-id".into(),
            version: "v42".into(),
            size: 20 << 30,
        };
        let b = encode_file(&f).unwrap();
        assert_eq!(Some(Identity::File(f.clone())), decode(&b));
        assert_eq!(Some(Identity::Dir), decode(&encode_dir()));
        assert_eq!(None, decode(&b[..b.len() - 1]));
        assert_eq!(None, decode(&[b.as_slice(), b"x"].concat()));
        assert_eq!(None, decode(&[]));
        assert_eq!(None, decode(&[9]));
        let huge = FileRef {
            id: "x".repeat(MAX),
            ..f
        };
        assert_eq!(None, encode_file(&huge));
    }
}
