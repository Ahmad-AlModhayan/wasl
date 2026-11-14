//! Helpers for canonical keys: `/`-separated relative paths without a leading slash, and for
//! comparing a pair's folder with others.

use std::path::{Path, PathBuf};

/// `"a/b/c"` → `"a/b"`; `"a"` → `""`.
pub fn parent_of(key: &str) -> &str {
    key.rsplit_once('/').map_or("", |(parent, _)| parent)
}

/// `"a/b/c"` → `"c"`.
pub fn name_of(key: &str) -> &str {
    key.rsplit_once('/').map_or(key, |(_, name)| name)
}

/// Number of `/` in the key: 0 for top-level entries.
pub fn depth(key: &str) -> usize {
    key.bytes().filter(|&b| b == b'/').count()
}

/// A pair's folder as a path to compare with others: a full path as it is, a folder picked with
/// Android's folder picker from the phone's own storage as the path it stands for (so it meets a
/// path pair on the same files), any other picked folder as its URI. None when no folder was
/// chosen.
pub fn comparable(local_root: &str) -> Option<PathBuf> {
    if let Some(path) = android_storage_path(local_root) {
        return Some(PathBuf::from(path));
    }
    if local_root.starts_with("content://") {
        return Some(PathBuf::from(local_root.trim_end_matches('/')));
    }
    let p = Path::new(local_root);
    p.is_absolute().then(|| p.to_path_buf())
}

/// The path behind a tree URI from Android's own storage provider
/// (`content://com.android.externalstorage.documents/tree/primary%3ADCIM%2FCamera` is
/// `/storage/emulated/0/DCIM/Camera`), or None for any other.
fn android_storage_path(uri: &str) -> Option<String> {
    let rest = uri.strip_prefix("content://com.android.externalstorage.documents/tree/")?;
    let id = percent_decode(rest.split('/').next().unwrap_or(""))?;
    let (volume, rel) = id.split_once(':').unwrap_or((&id, ""));
    let base = if volume == "primary" {
        "/storage/emulated/0".to_owned()
    } else {
        format!("/storage/{volume}")
    };
    let rel = rel.trim_matches('/');
    Some(if rel.is_empty() {
        base
    } else {
        format!("{base}/{rel}")
    })
}

fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}
