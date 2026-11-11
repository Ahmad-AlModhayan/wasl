//! `.waslignore` and `.gitignore` files at any depth, presets and the 1.1.4 pair patterns
//! (SRS FR-G1, FR-G2, FR-G3, FR-G6, FR-G8).
//!
//! Rules match canonical keys (Drive names, `/`-separated), so every device in a group reaches the
//! same verdict for the same file whatever its OS does to names. Matching is pure: callers hand in
//! the contents of the rule files they found (the scanner reads them).
//!
//! Precedence, as in git: rule files nearer to the path win over files higher up; within one
//! directory `.waslignore` wins over `.gitignore`; within one file the last matching line wins;
//! presets sit below every file. A path inside an ignored folder is ignored, and a negation cannot
//! re-include it (git cannot either). The 1.1.4 pair patterns apply on top and can only exclude.

use crate::legacy::{PairRule, PathFilter};
use crate::presets::Preset;
use ignore::Match;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use std::collections::BTreeMap;
use std::fmt;

pub const WASLIGNORE: &str = ".waslignore";
pub const GITIGNORE: &str = ".gitignore";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum FileKind {
    WaslIgnore,
    GitIgnore,
}

impl FileKind {
    pub fn file_name(self) -> &'static str {
        match self {
            FileKind::WaslIgnore => WASLIGNORE,
            FileKind::GitIgnore => GITIGNORE,
        }
    }
}

/// The rule-file kind for a file name, if it is one.
pub fn rule_file_kind(name: &str) -> Option<FileKind> {
    match name {
        WASLIGNORE => Some(FileKind::WaslIgnore),
        GITIGNORE => Some(FileKind::GitIgnore),
        _ => None,
    }
}

/// Where a deciding rule came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleOrigin {
    /// A rule file; `file` is its key, e.g. `app/.waslignore`. `line` is 1-based.
    File {
        file: String,
        line: usize,
    },
    Preset {
        preset: Preset,
        line: usize,
    },
    /// The pair's include/exclude patterns from 1.1.4.
    PairPatterns,
    /// A rule matched but its origin could not be recovered. The decision still stands.
    Unknown,
}

impl fmt::Display for RuleOrigin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RuleOrigin::File { file, line } => write!(f, "{file}:{line}"),
            RuleOrigin::Preset { preset, line } => write!(f, "{preset}:{line}"),
            RuleOrigin::PairPatterns => f.write_str("pair patterns"),
            RuleOrigin::Unknown => f.write_str("unknown rule"),
        }
    }
}

/// The rule that decided a path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleMatch {
    pub origin: RuleOrigin,
    /// The line as written, e.g. `node_modules/` or `!keep.log`.
    pub pattern: String,
    /// The decision was made for this ancestor folder rather than the path itself.
    pub via_parent: Option<String>,
    /// For [`RuleOrigin::PairPatterns`], the pair setting that decided.
    pub pair_rule: Option<PairRule>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    pub ignored: bool,
    /// The deciding rule. A path no rule mentions has `None`; a path re-included by `!pattern`
    /// has `Some` with `ignored == false`.
    pub rule: Option<RuleMatch>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleError {
    pub origin: RuleOrigin,
    pub message: String,
}

impl fmt::Display for RuleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.origin, self.message)
    }
}

impl std::error::Error for RuleError {}

/// Builds [`IgnoreRules`] from rule files, presets and pair patterns.
#[derive(Default, Clone)]
pub struct IgnoreRulesBuilder {
    files: BTreeMap<(String, FileKind), String>,
    presets: Vec<Preset>,
    honor_gitignore: bool,
    pair_patterns: Option<PathFilter>,
}

impl IgnoreRulesBuilder {
    /// Adds the contents of a rule file found in folder `dir_key` (`""` for the pair root).
    pub fn file(mut self, dir_key: &str, kind: FileKind, contents: &str) -> Self {
        self.files
            .insert((dir_key.trim_matches('/').to_owned(), kind), contents.to_owned());
        self
    }

    pub fn presets(mut self, presets: impl IntoIterator<Item = Preset>) -> Self {
        self.presets.extend(presets);
        self.presets.sort();
        self.presets.dedup();
        self
    }

    /// Whether `.gitignore` files count (per pair, SRS FR-G2). Off by default.
    pub fn honor_gitignore(mut self, honor: bool) -> Self {
        self.honor_gitignore = honor;
        self
    }

    /// The 1.1.4 include/exclude patterns of the pair (SRS FR-G8).
    pub fn pair_patterns(mut self, filter: PathFilter) -> Self {
        self.pair_patterns = Some(filter);
        self
    }

    /// Compiles every rule. An invalid line is reported with its file and line.
    pub fn build(self) -> Result<IgnoreRules, RuleError> {
        let mut origins: Vec<RuleOrigin> = Vec::new();
        let mut levels: BTreeMap<String, Vec<Level>> = BTreeMap::new();
        for ((dir, kind), contents) in &self.files {
            if *kind == FileKind::GitIgnore && !self.honor_gitignore {
                continue;
            }
            let file = if dir.is_empty() {
                kind.file_name().to_owned()
            } else {
                format!("{dir}/{}", kind.file_name())
            };
            let lines = contents.lines().enumerate().map(|(i, l)| (i + 1, l));
            let gi = compile(&mut origins, lines, |line| RuleOrigin::File {
                file: file.clone(),
                line,
            })?;
            levels
                .entry(dir.clone())
                .or_default()
                .push(Level { kind: *kind, gi });
        }
        // Within a folder `.waslignore` is consulted before `.gitignore`.
        for files in levels.values_mut() {
            files.sort_by_key(|l| l.kind);
        }
        let mut presets = Vec::new();
        for preset in self.presets {
            let lines = preset.lines().iter().enumerate().map(|(i, l)| (i + 1, *l));
            presets.push(compile(&mut origins, lines, |line| RuleOrigin::Preset {
                preset,
                line,
            })?);
        }
        Ok(IgnoreRules {
            origins,
            levels,
            presets,
            pair_patterns: self.pair_patterns,
        })
    }
}

struct Level {
    kind: FileKind,
    gi: Gitignore,
}

/// Each line is added with an index into `origins` as its "from" path, so a match reports file and
/// line. Only the index travels through the `ignore` crate: no user text is parsed back.
fn compile<'a>(
    origins: &mut Vec<RuleOrigin>,
    lines: impl Iterator<Item = (usize, &'a str)>,
    origin: impl Fn(usize) -> RuleOrigin,
) -> Result<Gitignore, RuleError> {
    let mut b = GitignoreBuilder::new("");
    b.case_insensitive(false).map_err(|e| RuleError {
        origin: origin(0),
        message: e.to_string(),
    })?;
    for (n, line) in lines {
        let index = origins.len();
        origins.push(origin(n));
        b.add_line(Some(index.to_string().into()), line)
            .map_err(|e| RuleError {
                origin: origin(n),
                message: e.to_string(),
            })?;
    }
    b.build().map_err(|e| RuleError {
        origin: origin(0),
        message: e.to_string(),
    })
}

/// Compiled rules for one pair.
pub struct IgnoreRules {
    /// Origin of every compiled line, by the index stored in the line's `from` field.
    origins: Vec<RuleOrigin>,
    /// Rule files by the folder (key) they sit in.
    levels: BTreeMap<String, Vec<Level>>,
    presets: Vec<Gitignore>,
    pair_patterns: Option<PathFilter>,
}

impl IgnoreRules {
    pub fn builder() -> IgnoreRulesBuilder {
        IgnoreRulesBuilder::default()
    }

    /// True when the key must stay out of every route (SRS FR-G4).
    pub fn is_ignored(&self, key: &str, is_dir: bool) -> bool {
        self.check(key, is_dir).ignored
    }

    /// The verdict for `key` and the rule that decided it (`wasl ignore --explain`, SRS FR-G6).
    pub fn check(&self, key: &str, is_dir: bool) -> Verdict {
        let key = key.trim_matches('/');
        if key.is_empty() {
            return Verdict {
                ignored: false,
                rule: None,
            };
        }
        // Rule files always sync, so every device applies the same rules (SRS FR-G7).
        let is_rule_file = !is_dir && rule_file_kind(name_of(key)).is_some();

        // A folder that is ignored takes everything below it along.
        for (i, _) in key.match_indices('/') {
            let parent = &key[..i];
            if let Some(m) = self.decide(parent, true).filter(|m| m.0) {
                return Verdict {
                    ignored: true,
                    rule: Some(RuleMatch {
                        via_parent: Some(parent.to_owned()),
                        ..m.1
                    }),
                };
            }
            if let Some(why) = self.pair_patterns.as_ref().and_then(|f| f.rejects(parent, true)) {
                return Verdict {
                    ignored: true,
                    rule: Some(pair_rule(Some(parent), why)),
                };
            }
        }
        if is_rule_file {
            return Verdict {
                ignored: false,
                rule: None,
            };
        }
        let own = self.decide(key, is_dir);
        if let Some((true, rule)) = own {
            return Verdict {
                ignored: true,
                rule: Some(rule),
            };
        }
        if let Some(why) = self.pair_patterns.as_ref().and_then(|f| f.rejects(key, is_dir)) {
            return Verdict {
                ignored: true,
                rule: Some(pair_rule(None, why)),
            };
        }
        Verdict {
            ignored: false,
            rule: own.map(|(_, rule)| rule),
        }
    }

    fn verdict_of(&self, m: Match<&ignore::gitignore::Glob>) -> Option<(bool, RuleMatch)> {
        let (ignored, glob) = match m {
            Match::None => return None,
            Match::Ignore(g) => (true, g),
            Match::Whitelist(g) => (false, g),
        };
        // Fail closed: a match whose origin can't be found still decides.
        let origin = glob
            .from()
            .and_then(|p| p.to_str())
            .and_then(|i| i.parse::<usize>().ok())
            .and_then(|i| self.origins.get(i).cloned())
            .unwrap_or(RuleOrigin::Unknown);
        Some((
            ignored,
            RuleMatch {
                origin,
                pattern: glob.original().to_owned(),
                via_parent: None,
                pair_rule: None,
            },
        ))
    }

    /// The decision for `path` alone (ancestors not considered): nearest rule file first, then presets.
    fn decide(&self, path: &str, is_dir: bool) -> Option<(bool, RuleMatch)> {
        // Folders that can hold rules for `path`, deepest first: "a/b", "a", "".
        let mut dirs: Vec<&str> = path.match_indices('/').map(|(i, _)| &path[..i]).collect();
        dirs.reverse();
        dirs.push("");
        for dir in dirs {
            let Some(files) = self.levels.get(dir) else {
                continue;
            };
            let rel = if dir.is_empty() {
                path
            } else {
                &path[dir.len() + 1..]
            };
            for level in files {
                if let Some(d) = self.verdict_of(level.gi.matched(rel, is_dir)) {
                    return Some(d);
                }
            }
        }
        self.presets
            .iter()
            .find_map(|gi| self.verdict_of(gi.matched(path, is_dir)))
    }
}

fn pair_rule(via_parent: Option<&str>, why: PairRule) -> RuleMatch {
    RuleMatch {
        origin: RuleOrigin::PairPatterns,
        pattern: String::new(),
        via_parent: via_parent.map(str::to_owned),
        pair_rule: Some(why),
    }
}

fn name_of(key: &str) -> &str {
    key.rsplit_once('/').map_or(key, |(_, name)| name)
}
