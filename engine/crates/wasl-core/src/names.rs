//! Drive names → names the local file system accepts (SRS FR-C7).
//!
//! Google Drive accepts almost any name, including `/`. Android external storage
//! (FUSE/MediaProvider) and Windows reject `" * : < > ? \ |` and control characters. Such names are
//! mapped to full-width look-alikes when materialised locally; the original Drive name stays the
//! key, so nothing is ever renamed on Drive. The result is always a single path component: `/` is
//! mapped too (1.1.4 did not, so a Drive name containing `/` could point outside the pair).
//!
//! Lengths that 1.1.4 measured in UTF-16 code units (Kotlin `String.length`) are measured the same
//! way here so both engines map every name identically.

/// Longest name, in UTF-8 bytes, that the local file systems we target accept.
pub const MAX_NAME_BYTES: usize = 255;

fn full_width(c: char) -> Option<char> {
    Some(match c {
        '"' => '＂',
        '*' => '＊',
        ':' => '：',
        '<' => '＜',
        '>' => '＞',
        '?' => '？',
        '\\' => '＼',
        '|' => '｜',
        '/' => '／',
        _ => return None,
    })
}

fn is_control(c: char) -> bool {
    (c as u32) < 0x20
}

fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

fn map_chars(s: &str) -> String {
    s.chars()
        .map(|c| match full_width(c) {
            Some(mapped) => mapped,
            None if is_control(c) => '␀',
            None => c,
        })
        .collect()
}

/// True when `name` cannot be used as-is for a local file.
pub fn needs_mapping(name: &str) -> bool {
    name.chars().any(|c| full_width(c).is_some() || is_control(c))
        || name == "."
        || name == ".."
        || name.is_empty()
        || name.len() > MAX_NAME_BYTES
}

/// The local name for a Drive name. Returns `remote_name` unchanged when no mapping is needed.
pub fn to_local_name(remote_name: &str) -> String {
    if !needs_mapping(remote_name) {
        return remote_name.to_owned();
    }
    let mut out = map_chars(remote_name);
    if out == "." || out == ".." {
        out.insert(0, '＿');
    }
    if out.is_empty() {
        out.push_str("＿unnamed");
    }
    // Keep under the byte limit while preserving the extension when possible.
    if out.len() > MAX_NAME_BYTES {
        let ext = out.rsplit_once('.').map_or("", |(_, ext)| ext);
        let ext_part = if !ext.is_empty() && utf16_len(ext) < 16 {
            format!(".{ext}")
        } else {
            String::new()
        };
        let mut base = out.strip_suffix(ext_part.as_str()).unwrap_or(&out).to_owned();
        while base.len() + ext_part.len() > MAX_NAME_BYTES && base.pop().is_some() {}
        out = base + &ext_part;
    }
    out
}

/// Conflict copy naming: `report (conflicted copy from Pixel, 2026-09-30 14-03).docx`.
pub fn conflict_name(name: &str, device: &str, stamp: &str) -> String {
    numbered_conflict_name(name, device, stamp, 1)
}

/// The `n`th conflict copy of one file within one stamp's minute: the first is
/// [`conflict_name`], later ones end in `, n`: `report (conflicted copy from Pixel, 2026-09-30 14-03, 2).docx`.
pub fn numbered_conflict_name(name: &str, device: &str, stamp: &str, n: u32) -> String {
    let stamp = if n > 1 {
        format!("{stamp}, {n}")
    } else {
        stamp.to_owned()
    };
    conflict_name_with(name, device, &stamp)
}

fn conflict_name_with(name: &str, device: &str, stamp: &str) -> String {
    let (base, ext) = match name.rfind('.') {
        Some(dot) if dot > 0 && dot < name.len() - 1 && utf16_len(&name[dot..]) <= 12 => name.split_at(dot),
        _ => (name, ""),
    };
    // device and stamp come from settings and the clock, not from a checked file name
    let (device, stamp) = (map_chars(device), map_chars(stamp));
    format!("{base} (conflicted copy from {device}, {stamp}){ext}")
}
