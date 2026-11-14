//! The include/exclude patterns of Drive Sync 1.1.4, kept working as pair-level rules (SRS FR-G8).
//!
//! Glob syntax (case-insensitive):
//! - `?` one character except `/`; `*` any characters except `/`; `**` any characters including `/`
//! - `[abc]` character class, `[!abc]` or `[^abc]` negated, ranges allowed
//! - `{1..9}` numeric range (at most 2000 values), `{a,b}` alternation
//!
//! A pattern without a leading `/` is implicitly prefixed with `**/` so it matches at any depth; a
//! leading `/` anchors it to the pair root. A trailing `/` matches directories only.

use regex::Regex;
use std::fmt;
use std::sync::LazyLock;

/// Suffixes of files the engine creates itself and must never sync.
pub const INTERNAL_SUFFIXES: [&str; 2] = [".drivesync-part", ".drivesync-tmp"];

const MAX_RANGE_VALUES: u64 = 2000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GlobError {
    Empty,
    Invalid(String),
}

impl fmt::Display for GlobError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GlobError::Empty => f.write_str("empty pattern"),
            GlobError::Invalid(why) => write!(f, "invalid pattern: {why}"),
        }
    }
}

impl std::error::Error for GlobError {}

#[derive(Debug, Clone)]
pub struct Glob {
    pattern: String,
    regex: Regex,
    dir_only: bool,
}

impl Glob {
    pub fn compile(raw: &str) -> Result<Glob, GlobError> {
        let mut p = raw.trim();
        if p.is_empty() {
            return Err(GlobError::Empty);
        }
        let dir_only = p.ends_with('/');
        p = p.trim_end_matches('/');
        let anchored = p.starts_with('/');
        p = p.trim_start_matches('/');
        let body = if !anchored && !p.starts_with("**/") {
            format!("**/{p}")
        } else {
            p.to_owned()
        };
        let regex =
            Regex::new(&format!("(?i){}", to_regex(&body))).map_err(|e| GlobError::Invalid(e.to_string()))?;
        Ok(Glob {
            pattern: raw.to_owned(),
            regex,
            dir_only,
        })
    }

    /// The pattern as the user wrote it.
    pub fn pattern(&self) -> &str {
        &self.pattern
    }

    pub fn dir_only(&self) -> bool {
        self.dir_only
    }

    /// `rel_path` is relative to the pair root, `/`-separated; a leading `/` is ignored.
    pub fn matches(&self, rel_path: &str, is_dir: bool) -> bool {
        if self.dir_only && !is_dir {
            return false;
        }
        self.regex.is_match(rel_path.trim_start_matches('/'))
    }
}

static RANGE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*([0-9]+)\s*\.\.\s*([0-9]+)\s*$").expect("static regex"));

fn to_regex(p: &str) -> String {
    let chars: Vec<char> = p.chars().collect();
    let find = |from: usize, target: char| chars[from..].iter().position(|&c| c == target).map(|i| i + from);
    let mut out = String::from("^");
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '*' if chars.get(i + 1) == Some(&'*') => {
                if chars.get(i + 2) == Some(&'/') {
                    // "**/" also matches zero directories
                    out.push_str("(?:.*/)?");
                    i += 3;
                } else {
                    out.push_str(".*");
                    i += 2;
                }
                continue;
            }
            '*' => out.push_str("[^/]*"),
            '?' => out.push_str("[^/]"),
            '[' => {
                let Some(end) = find(i + 1, ']') else {
                    out.push_str(r"\[");
                    i += 1;
                    continue;
                };
                let mut class: String = chars[i + 1..end].iter().collect();
                let negated = class.starts_with('!') || class.starts_with('^');
                if negated {
                    class.remove(0);
                }
                out.push('[');
                if negated {
                    out.push('^');
                }
                out.push_str(&class.replace('\\', r"\\"));
                out.push(']');
                i = end + 1;
                continue;
            }
            '{' => {
                if let Some(end) = find(i + 1, '}') {
                    let body: String = chars[i + 1..end].iter().collect();
                    if let Some(alt) = brace_alternatives(&body) {
                        out.push_str(&alt);
                        i = end + 1;
                        continue;
                    }
                }
                out.push_str(r"\{");
            }
            c => out.push_str(&regex::escape(c.encode_utf8(&mut [0; 4]))),
        }
        i += 1;
    }
    out.push('$');
    out
}

/// `{lo..hi}` → every integer in the range; `{a,b}` → alternation; anything else → `None` (literal).
fn brace_alternatives(body: &str) -> Option<String> {
    if let Some(caps) = RANGE.captures(body) {
        let a: Option<u64> = caps[1].parse().ok();
        let b: Option<u64> = caps[2].parse().ok();
        if let (Some(a), Some(b)) = (a, b) {
            let (lo, hi) = (a.min(b), a.max(b));
            if hi - lo <= MAX_RANGE_VALUES {
                // parity: 1.1.4 drops leading zeros, so {01..03} matches 1, 2, 3 and not 01.
                let options: Vec<String> = (lo..=hi).map(|n| n.to_string()).collect();
                return Some(format!("(?:{})", options.join("|")));
            }
        }
    }
    if body.contains(',') {
        let options: Vec<String> = body.split(',').map(regex::escape).collect();
        return Some(format!("(?:{})", options.join("|")));
    }
    None
}

/// Include/exclude evaluation with directory pruning semantics.
#[derive(Debug, Clone)]
pub struct PathFilter {
    includes: Vec<Glob>,
    excludes: Vec<Glob>,
    /// Lower-cased for matching, and as the user wrote it.
    excluded_dirs: Vec<(String, String)>,
    sync_hidden: bool,
}

impl PathFilter {
    /// Blank or invalid patterns are ignored, as in 1.1.4.
    pub fn new<S: AsRef<str>>(
        includes: &[S],
        excludes: &[S],
        excluded_subfolders: &[S],
        sync_hidden: bool,
    ) -> PathFilter {
        let compile = |patterns: &[S]| {
            patterns
                .iter()
                .filter_map(|p| Glob::compile(p.as_ref()).ok())
                .collect()
        };
        PathFilter {
            includes: compile(includes),
            excludes: compile(excludes),
            excluded_dirs: excluded_subfolders
                .iter()
                .map(|d| d.as_ref().trim_matches('/'))
                .filter(|d| !d.is_empty())
                .map(|d| (d.to_lowercase(), d.to_owned()))
                .collect(),
            sync_hidden,
        }
    }

    /// `true` keeps the entry. For a directory, `false` prunes its whole subtree.
    pub fn accept(&self, rel_path: &str, is_dir: bool) -> bool {
        self.rejects(rel_path, is_dir).is_none()
    }

    /// The pair setting that leaves the entry out, or `None` to keep it. The scan and
    /// `wasl ignore --explain` both decide with this, so they cannot disagree.
    pub fn rejects(&self, rel_path: &str, is_dir: bool) -> Option<PairRule> {
        let rel = rel_path.trim_matches('/');
        if rel.is_empty() {
            return None;
        }
        let name = rel.rsplit('/').next().unwrap_or(rel);
        if INTERNAL_SUFFIXES.iter().any(|s| name.ends_with(s)) {
            return Some(PairRule::Internal);
        }
        if !self.sync_hidden && rel.split('/').any(|part| part.starts_with('.')) {
            return Some(PairRule::Hidden);
        }
        let lower = rel.to_lowercase();
        if let Some((_, d)) = self.excluded_dirs.iter().find(|(d, _)| {
            lower == *d
                || lower
                    .strip_prefix(d.as_str())
                    .is_some_and(|rest| rest.starts_with('/'))
        }) {
            return Some(PairRule::ExcludedSubfolder(d.clone()));
        }
        if let Some(g) = self.excludes.iter().find(|g| g.matches(rel, is_dir)) {
            return Some(PairRule::Exclude(g.pattern().to_owned()));
        }
        if !self.includes.is_empty() && !is_dir && !self.includes.iter().any(|g| g.matches(rel, false)) {
            return Some(PairRule::NotIncluded);
        }
        None
    }
}

/// Which of the pair's own settings left a path out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairRule {
    /// Wasl's own partial and temporary files.
    Internal,
    /// The pair skips hidden files and folders (names starting with a dot).
    Hidden,
    /// A subfolder left out of the pair (selective sync), as the pair names it.
    ExcludedSubfolder(String),
    /// One of the pair's exclude patterns, as written.
    Exclude(String),
    /// The pair has include patterns and none matches this file.
    NotIncluded,
}

impl std::fmt::Display for PairRule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PairRule::Internal => f.write_str("Wasl's own partial and temporary files are skipped"),
            PairRule::Hidden => f.write_str("hidden files and folders are skipped"),
            PairRule::ExcludedSubfolder(d) => write!(f, "the subfolder {d} is left out"),
            PairRule::Exclude(p) => write!(f, "the exclude pattern {p}"),
            PairRule::NotIncluded => f.write_str("no include pattern matches it"),
        }
    }
}
