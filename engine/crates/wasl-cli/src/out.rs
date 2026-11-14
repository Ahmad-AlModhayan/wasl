//! Plain output: aligned tables, local times, sizes, and colour only on a terminal.

use std::io::{IsTerminal, Write};
use std::sync::atomic::{AtomicUsize, Ordering};

/// Whether stdout is a terminal (progress redraws in place, colours allowed).
pub fn tty() -> bool {
    std::io::stdout().is_terminal()
}

/// Colours on a terminal only, never with `NO_COLOR`, and on Windows only where escape codes are
/// known to work (Windows Terminal, VS Code), since the classic console prints them raw.
fn colour_on() -> bool {
    if !tty() || std::env::var_os("NO_COLOR").is_some() {
        return false;
    }
    if cfg!(windows) {
        std::env::var_os("WT_SESSION").is_some() || std::env::var_os("TERM_PROGRAM").is_some()
    } else {
        std::env::var("TERM").map(|t| t != "dumb").unwrap_or(false)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Good,
    Busy,
    Bad,
    Quiet,
}

pub fn paint(text: &str, tone: Tone) -> String {
    let code = match tone {
        Tone::Good => "32",
        Tone::Busy => "36",
        Tone::Bad => "31",
        Tone::Quiet => "2",
    };
    if colour_on() {
        format!("\x1b[{code}m{text}\x1b[0m")
    } else {
        text.to_owned()
    }
}

/// Visible width, ignoring colour codes.
fn width(s: &str) -> usize {
    let mut n = 0;
    let mut in_escape = false;
    for c in s.chars() {
        if in_escape {
            if c == 'm' {
                in_escape = false;
            }
        } else if c == '\x1b' {
            in_escape = true;
        } else if !is_combining(c) {
            n += 1;
        }
    }
    n
}

/// Arabic marks (shadda, harakat) take no column.
fn is_combining(c: char) -> bool {
    matches!(c, '\u{064B}'..='\u{065F}' | '\u{0670}' | '\u{0300}'..='\u{036F}' | '\u{200E}' | '\u{200F}')
}

pub use wasl_cli::i18n::clean;

/// A tag only this process knows (random per run), marking table cells this program painted.
fn style_tag() -> &'static str {
    use std::hash::{BuildHasher, Hasher};
    static TAG: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    TAG.get_or_init(|| {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u64(now_ms() as u64);
        format!("\u{E000}{:016x}\u{E001}", h.finish())
    })
}

/// A table cell of the program's own text in a colour. Every other cell is cleaned as data.
pub fn styled(text: &str, tone: Tone) -> String {
    format!("{}{}", style_tag(), paint(text, tone))
}

/// Rows padded into columns two spaces apart; the first row is the header. Cells are data and
/// pass through [`clean`], except cells made with [`styled`].
pub fn table(rows: &[Vec<String>]) -> String {
    let rows: Vec<Vec<String>> = rows
        .iter()
        .map(|r| {
            r.iter()
                .map(|c| match c.strip_prefix(style_tag()) {
                    Some(own) => own.to_owned(),
                    None => clean(c),
                })
                .collect()
        })
        .collect();
    let cols = rows.iter().map(Vec::len).max().unwrap_or(0);
    let mut widths = vec![0; cols];
    for row in &rows {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(width(cell));
        }
    }
    let mut s = String::new();
    for (r, row) in rows.iter().enumerate() {
        let mut line = String::new();
        for (i, cell) in row.iter().enumerate() {
            let cell = if r == 0 {
                paint(cell, Tone::Quiet)
            } else {
                cell.clone()
            };
            line.push_str(&cell);
            if i + 1 < row.len() {
                line.push_str(&" ".repeat(widths[i] - width(&cell) + 2));
            }
        }
        s.push_str(line.trim_end());
        s.push('\n');
    }
    s
}

/// `YYYY-MM-DD HH:MM` in local time, from milliseconds since the epoch.
pub fn when(ms: i64) -> String {
    use chrono::{Local, TimeZone};
    match Local.timestamp_millis_opt(ms) {
        chrono::LocalResult::Single(t) | chrono::LocalResult::Ambiguous(t, _) => {
            t.format("%Y-%m-%d %H:%M").to_string()
        }
        chrono::LocalResult::None => "-".into(),
    }
}

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// Bytes as B, KB, MB or GB (1024-based, one decimal from KB up).
pub fn size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut v = bytes as f64 / 1024.0;
    let mut unit = 0;
    while v >= 1024.0 && unit + 1 < UNITS.len() {
        v /= 1024.0;
        unit += 1;
    }
    format!("{v:.1} {}", UNITS[unit])
}

/// Shortens a long id for tables; `wasl` accepts any unique prefix of 4+ characters.
pub fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

static PROGRESS_WIDTH: AtomicUsize = AtomicUsize::new(0);

/// Redraws one progress line in place (terminal only).
pub fn progress_line(text: &str) {
    let text = &clean(text);
    let w = width(text);
    let old = PROGRESS_WIDTH.swap(w.max(1), Ordering::SeqCst);
    let pad = " ".repeat(old.saturating_sub(w));
    let mut out = std::io::stdout().lock();
    let _ = write!(out, "\r{text}{pad}");
    let _ = out.flush();
}

/// Ends a progress line drawn by [`progress_line`], so the next output starts on its own line.
pub fn finish_progress_line() {
    if PROGRESS_WIDTH.swap(0, Ordering::SeqCst) > 0 {
        println!();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!("512 B", size(512));
        assert_eq!("1.5 KB", size(1536));
        assert_eq!("2.0 MB", size(2 * 1024 * 1024));
        assert_eq!("3.2 GB", size(3_435_973_837));
    }

    #[test]
    fn tables_align_by_visible_width() {
        let t = table(&[
            vec!["A".into(), "B".into()],
            vec!["محدّث".into(), "x".into()],
            vec!["long name".into(), "y".into()],
        ]);
        let lines: Vec<&str> = t.lines().collect();
        assert_eq!("A          B", lines[0]);
        // The shadda takes no column.
        assert_eq!("محدّث       x", lines[1]);
        assert_eq!("long name  y", lines[2]);
    }

    #[test]
    fn times_are_minutes() {
        let s = when(1_760_000_000_000);
        assert_eq!(16, s.len());
        assert_eq!(b'-', s.as_bytes()[4]);
        assert_eq!(b':', s.as_bytes()[13]);
    }
}

#[cfg(test)]
mod clean_tests {
    use super::{clean, table};

    #[test]
    fn control_sequences_from_outside_are_neutralised() {
        // OSC (retitle the window), screen clear, C1 CSI and a right-to-left override.
        assert_eq!("?]0;pwned?x", clean("\u{1b}]0;pwned\u{7}x"));
        assert_eq!("?[2Jgone", clean("\u{1b}[2Jgone"));
        assert_eq!("a?b", clean("a\u{9b}b"));
        assert_eq!("report?gpj.exe", clean("report\u{202E}gpj.exe"));
        // Colour codes from outside (here "conceal") and fake extra lines are data too.
        assert_eq!("?[8mhidden", clean("\u{1b}[8mhidden"));
        assert_eq!("a?fake line", clean("a\nfake line"));
        assert_eq!("zero?width?mark", clean("zero\u{200B}width\u{200F}mark"));
        assert!(!table(&[vec!["h".into()], vec!["\u{1b}]0;x\u{7}".into()]]).contains("]0;x\u{7}"));
        assert_eq!("ملف.txt", clean("ملف.txt"));
    }

    #[test]
    fn only_cells_the_program_painted_keep_colour() {
        let own = super::styled("ok", super::Tone::Good);
        let t = table(&[
            vec!["h".into(), "h".into()],
            vec![own, "\u{1b}[31mfake\u{1b}[0m".into()],
        ]);
        assert!(t.contains("ok"));
        assert!(t.contains("?[31mfake?[0m"), "{t:?}");
        // The tag a styled cell carries is unknown outside this process.
        assert!(!t.contains('\u{E000}'));
    }
}
