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

/// What the disk a name lands on refuses. The one function that says what local name a remote name
/// gets ([`to_local_name_with`]) takes this, so a test on any OS can switch Windows' rules on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NameRules {
    /// Windows' rules: reserved device names (`CON`, `aux.txt`), a trailing dot or space, `:`, and
    /// names that look like 8.3 short names.
    pub windows: bool,
    /// The characters Drive allows and Android and Windows refuse, `.`, `..`, an empty name and
    /// one that is too long are mapped.
    pub reserved_chars: bool,
}

impl NameRules {
    /// What every Drive name gets on every device: the reserved characters only.
    pub const PORTABLE: NameRules = NameRules {
        windows: false,
        reserved_chars: true,
    };

    /// Drive names landing on a disk: the reserved characters, and Windows' rules when `windows`.
    pub const fn drive(windows: bool) -> NameRules {
        NameRules {
            windows,
            reserved_chars: true,
        }
    }

    /// Names another device sent, which that device's disk already accepted: nothing is mapped
    /// unless this disk is Windows', so a name that is fine here is never renamed here.
    pub const fn device(windows: bool) -> NameRules {
        NameRules {
            windows,
            reserved_chars: windows,
        }
    }
}

impl Default for NameRules {
    fn default() -> Self {
        NameRules::PORTABLE
    }
}

/// True when `name` cannot be used as-is for a local file.
pub fn needs_mapping(name: &str) -> bool {
    needs_mapping_with(name, NameRules::PORTABLE)
}

/// [`needs_mapping`] under `rules`.
pub fn needs_mapping_with(name: &str, rules: NameRules) -> bool {
    (rules.reserved_chars
        && (name.chars().any(|c| full_width(c).is_some() || is_control(c))
            || name == "."
            || name == ".."
            || name.is_empty()
            || name.len() > MAX_NAME_BYTES))
        || (rules.windows && windows_refuses(name))
}

/// The local name for a Drive name. Returns `remote_name` unchanged when no mapping is needed.
pub fn to_local_name(remote_name: &str) -> String {
    to_local_name_with(remote_name, NameRules::PORTABLE)
}

/// The one function that says what local name a remote name gets. Pure and deterministic, so every
/// device that applies the same name under the same rules gets the same local name.
pub fn to_local_name_with(remote_name: &str, rules: NameRules) -> String {
    if !needs_mapping_with(remote_name, rules) {
        return remote_name.to_owned();
    }
    let mut out = if rules.reserved_chars {
        map_chars(remote_name)
    } else {
        remote_name.to_owned()
    };
    if out == "." || out == ".." {
        out.insert(0, '＿');
    }
    if out.is_empty() {
        out.push_str("＿unnamed");
    }
    if !rules.reserved_chars {
        return if rules.windows { windows_safe(out) } else { out };
    }
    // Windows' rules first, on the whole name: a trailing dot or space becomes three bytes, so
    // the length is judged after them, not before.
    if rules.windows {
        out = windows_safe(out);
    }
    // Then keep under the byte limit, preserving the extension when possible. A cut can leave a
    // trailing dot or space (or a short-name look-alike), so the rules run again on the cut and
    // the cut is made shorter until the result fits. A result that fits is its own mapping:
    // `needs_mapping_with` says no to it.
    let mut limit = MAX_NAME_BYTES;
    loop {
        let cut = cut_to(&out, limit);
        let done = if rules.windows { windows_safe(cut) } else { cut };
        if done.len() <= MAX_NAME_BYTES || limit < 16 {
            return done;
        }
        limit = limit.saturating_sub(done.len() - MAX_NAME_BYTES);
    }
}

/// `name` cut to at most `limit` bytes, keeping a short extension when there is one.
fn cut_to(name: &str, limit: usize) -> String {
    if name.len() <= limit {
        return name.to_owned();
    }
    let ext = name.rsplit_once('.').map_or("", |(_, ext)| ext);
    let ext_part = if !ext.is_empty() && utf16_len(ext) < 16 {
        format!(".{ext}")
    } else {
        String::new()
    };
    let mut base = name.strip_suffix(ext_part.as_str()).unwrap_or(name).to_owned();
    while base.len() + ext_part.len() > limit && base.pop().is_some() {}
    base + &ext_part
}

/// What a name still is once mapped, when the disk will refuse it anyway.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameRefusal {
    /// A component longer than the disk's limit (255 bytes).
    TooLong,
    /// A component Windows still refuses.
    Reserved,
}

/// `None` when `name` (already mapped) is one the disk accepts under `rules`.
pub fn refusal(name: &str, rules: NameRules) -> Option<NameRefusal> {
    // Bytes, as in `needs_mapping_with`: a name that fits in bytes fits in UTF-16 units too.
    if name.len() > MAX_NAME_BYTES {
        return Some(NameRefusal::TooLong);
    }
    (rules.windows && windows_refuses(name)).then_some(NameRefusal::Reserved)
}

fn windows_refuses(name: &str) -> bool {
    name.contains(':')
        || name.ends_with('.')
        || name.ends_with(' ')
        || is_windows_device_name(name)
        || looks_like_short_name(name)
}

/// Maps what Windows refuses and the character mapping leaves: a device name gets `＿` after its
/// stem (`aux.txt` becomes `aux＿.txt`), a short-name look-alike its `~` made full-width, and a
/// trailing dot or space a look-alike.
fn windows_safe(name: String) -> String {
    let mut out = name;
    if is_windows_device_name(&out) {
        let at = out.find('.').unwrap_or(out.len());
        out.insert(at, '＿');
    }
    // The `~` that makes it a short name is the last one in the stem; more than one `~` would
    // leave a name that still looks like one after the first is replaced.
    while looks_like_short_name(&out) {
        let stem_end = out.rfind('.').unwrap_or(out.len());
        match out[..stem_end].rfind('~') {
            Some(at) => out.replace_range(at..at + 1, "～"),
            None => break,
        }
    }
    let kept = out.trim_end_matches(['.', ' ']).len();
    if kept < out.len() {
        let tail: String = out[kept..]
            .chars()
            .map(|c| if c == '.' { '．' } else { '␣' })
            .collect();
        out.truncate(kept);
        out.push_str(&tail);
    }
    out
}

/// `REPORT~1.DOC`: a stem of at most 8 characters with `~` and digits at its end, and an extension
/// of at most 3. On Windows such a name may refer to a different file's short name.
pub fn looks_like_short_name(part: &str) -> bool {
    let (stem, ext) = part.rsplit_once('.').unwrap_or((part, ""));
    let Some((head, digits)) = stem.rsplit_once('~') else {
        return false;
    };
    !head.is_empty()
        && !digits.is_empty()
        && digits.bytes().all(|b| b.is_ascii_digit())
        && stem.chars().count() <= 8
        && ext.chars().count() <= 3
}

/// `CON`, `PRN`, `AUX`, `NUL`, `COM0`-`COM9`, `LPT0`-`LPT9` (and the superscript-digit forms), with
/// or without an extension, in any case.
pub fn is_windows_device_name(part: &str) -> bool {
    let stem = part
        .split('.')
        .next()
        .unwrap_or(part)
        .trim_end_matches([' ', '.'])
        .to_ascii_uppercase();
    match stem.as_str() {
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$" => true,
        _ => {
            // Split after three characters, not three bytes: names are often not ASCII.
            let cut = stem.char_indices().nth(3).map_or(stem.len(), |(i, _)| i);
            let (prefix, digit) = stem.split_at(cut);
            (prefix == "COM" || prefix == "LPT")
                && matches!(
                    digit,
                    "0" | "1"
                        | "2"
                        | "3"
                        | "4"
                        | "5"
                        | "6"
                        | "7"
                        | "8"
                        | "9"
                        | "\u{b9}"
                        | "\u{b2}"
                        | "\u{b3}"
                )
        }
    }
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
