//! Walks a pair folder breadth-first, loading rule files as it goes so ignored folders are never
//! entered (a `node_modules` with 100,000 files costs one directory read).

use crate::local::{FsEntry, LocalFs};
use std::collections::VecDeque;
use std::io::{self, Read};
use wasl_core::LocalEntry;
use wasl_ignore::IgnoreRules;
use wasl_ignore::rules::{FileKind, IgnoreRulesBuilder, rule_file_kind};

/// What a scan found.
pub struct ScanOutcome {
    /// Every entry kept, by local relative path.
    pub entries: Vec<LocalEntry>,
    /// Folders that could not be listed this time. The engine leaves everything under them alone,
    /// so nothing there reads as deleted.
    pub unreadable: Vec<String>,
    /// The rules in force for this pair, including every rule file found.
    pub rules: IgnoreRules,
    /// Rule files found, as (folder, kind, contents).
    pub rule_files: Vec<(String, FileKind, String)>,
}

/// Largest rule file read; anything bigger is not a hand-written ignore file.
const MAX_RULE_FILE: u64 = 1024 * 1024;

/// Scans `fs`. `builder` carries the pair's presets and patterns; rule files found are added to it.
/// `key_of` maps a local relative path to its canonical key (names may be mapped), so rules see
/// the same keys the planner does. The pair root being unreadable fails the scan.
pub fn scan(
    fs: &dyn LocalFs,
    builder: IgnoreRulesBuilder,
    key_of: &dyn Fn(&str) -> String,
    cancelled: impl FnMut() -> bool,
) -> io::Result<ScanOutcome> {
    walk(fs, builder, key_of, String::new(), Vec::new(), cancelled)
}

/// [`scan`] of one folder of the pair and everything under it (`under` is pair-relative; empty is
/// the whole pair). The rule files of the folders above it are read first, since they apply inside
/// it. A folder the rules ignore, or one below an ignored folder, has no entries; so does one that
/// does not exist (yet, or any more).
pub fn scan_under(
    fs: &dyn LocalFs,
    mut builder: IgnoreRulesBuilder,
    key_of: &dyn Fn(&str) -> String,
    under: &str,
    cancelled: impl FnMut() -> bool,
) -> io::Result<ScanOutcome> {
    if under.is_empty() {
        return scan(fs, builder, key_of, cancelled);
    }
    let mut rule_files = Vec::new();
    let mut dir = String::new();
    let mut above = Vec::new();
    let mut missing = false;
    for part in under.split('/') {
        match fs.list(&dir) {
            Ok(children) => {
                load_rule_files(fs, &mut builder, key_of, &dir, &children, &mut rule_files);
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound && !dir.is_empty() => missing = true,
            Err(e) => return Err(e),
        }
        dir = join(&dir, part);
        above.push(dir.clone());
    }
    let rules = builder.clone().build().map_err(invalid)?;
    missing = missing || matches!(fs.stat(under), Ok(None));
    if missing || above.iter().any(|d| rules.is_ignored(&key_of(d), true)) {
        return Ok(ScanOutcome {
            entries: Vec::new(),
            unreadable: Vec::new(),
            rules,
            rule_files,
        });
    }
    walk(fs, builder, key_of, under.to_owned(), rule_files, cancelled)
}

/// Loads the rule files among `children` of `dir` into `builder`. Returns whether any was found.
fn load_rule_files(
    fs: &dyn LocalFs,
    builder: &mut IgnoreRulesBuilder,
    key_of: &dyn Fn(&str) -> String,
    dir: &str,
    children: &[FsEntry],
    rule_files: &mut Vec<(String, FileKind, String)>,
) -> bool {
    let mut added = false;
    for c in children.iter().filter(|c| !c.is_dir) {
        if let Some(kind) = rule_file_kind(&c.name) {
            let rel = join(dir, &c.name);
            if let Some(text) = read_rule_file(fs, &rel, c) {
                let dir_key = key_of(dir);
                *builder = builder.clone().file(&dir_key, kind, &text);
                rule_files.push((dir_key, kind, text));
                added = true;
            }
        }
    }
    added
}

fn walk(
    fs: &dyn LocalFs,
    mut builder: IgnoreRulesBuilder,
    key_of: &dyn Fn(&str) -> String,
    start: String,
    mut rule_files: Vec<(String, FileKind, String)>,
    mut cancelled: impl FnMut() -> bool,
) -> io::Result<ScanOutcome> {
    let mut rules = builder.clone().build().map_err(invalid)?;
    let mut entries = Vec::new();
    let mut unreadable = Vec::new();
    let mut queue: VecDeque<String> = VecDeque::from([start.clone()]);

    while let Some(dir) = queue.pop_front() {
        if cancelled() {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "scan cancelled"));
        }
        let children = match fs.list(&dir) {
            Ok(c) => c,
            Err(e) if dir == start => return Err(e),
            Err(_) => {
                unreadable.push(dir);
                continue;
            }
        };

        // Rule files in this folder apply to its children, so load them first.
        if load_rule_files(fs, &mut builder, key_of, &dir, &children, &mut rule_files) {
            rules = builder.clone().build().map_err(invalid)?;
        }

        for c in children {
            let rel = join(&dir, &c.name);
            if rules.is_ignored(&key_of(&rel), c.is_dir) {
                continue;
            }
            entries.push(LocalEntry {
                rel: rel.clone(),
                is_dir: c.is_dir,
                size: c.size,
                mtime: c.mtime_ms,
            });
            if c.is_dir {
                queue.push_back(rel);
            }
        }
    }
    Ok(ScanOutcome {
        entries,
        unreadable,
        rules,
        rule_files,
    })
}

fn read_rule_file(fs: &dyn LocalFs, rel: &str, e: &FsEntry) -> Option<String> {
    if e.size > MAX_RULE_FILE {
        return None;
    }
    let mut text = String::new();
    fs.open_read(rel)
        .ok()?
        .take(MAX_RULE_FILE)
        .read_to_string(&mut text)
        .ok()?;
    Some(text)
}

fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_owned()
    } else {
        format!("{dir}/{name}")
    }
}

fn invalid(e: wasl_ignore::rules::RuleError) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, e.to_string())
}
