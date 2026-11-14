//! Sync between any number of participants with version vectors (SRS FR-C4–C6,
//! docs/DEVICE_SYNC.md §4). Pure: no I/O. Each participant keeps an [`Index`] of its files; a scan
//! of the folder turns local edits into new versions ([`Index::apply_scan`]), and comparing with
//! another participant's index yields [`Step`]s to apply ([`plan_with`]).
//!
//! Rules, the same on every participant so they all end in one state:
//! - A version whose vector descends from ours replaces ours (edit, delete or folder change).
//! - Versions that changed independently but hold the same content merge without a conflict.
//! - Otherwise it is a conflict: a change beats a delete; between two different contents the newer
//!   modification time wins, then the higher author id, then the larger hash.
//! - **Only the participant holding the losing content resolves a conflict**: it keeps that
//!   content as a conflict copy ([`ConflictMode::KeepBoth`]) or sends it to the bin
//!   ([`ConflictMode::NewestWins`]), takes the winner and records the merged vector. A participant
//!   whose content wins does nothing; the merged version reaches it later from the loser's side.
//!   (If the winning side merged too, its merged vector would dominate the loser's version on a
//!   third participant, which would then overwrite the losing content without saving it. The
//!   property tests found exactly that.)
//! - Merging the same content keeps the later of the two authored times and its author on both
//!   sides, so equivalent copies of a version look alike to everyone who compares with them.
//! - Equal vectors with different content can still arise when resolutions race on different
//!   participants. That is repaired by a rule that looks at content only (a change beats a delete,
//!   then the larger hash wins), which every participant computes the same way; the losing
//!   content is kept like any conflict's.

use crate::vv::{Causality, ShortId, VersionVector};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

/// One version of a file or folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Version {
    pub is_dir: bool,
    pub deleted: bool,
    /// BLAKE3 of the content (hex); `None` for folders and deletions.
    pub hash: Option<String>,
    pub size: u64,
    /// The modification time the version was made with. Part of the version: it never changes
    /// while the vector stays the same, so every participant orders conflicts the same way.
    pub mtime: i64,
    pub vv: VersionVector,
    /// The participant that made this version (its last tick).
    pub author: ShortId,
}

impl Version {
    /// Same content: both gone, both folders, or files with one hash.
    pub fn same_content(&self, other: &Version) -> bool {
        match (self.deleted, other.deleted) {
            (true, true) => true,
            (false, false) => self.is_dir == other.is_dir && (self.is_dir || self.hash == other.hash),
            _ => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexEntry {
    pub version: Version,
    /// The owner's sequence number when this entry last changed (for "changes since N").
    pub seq: u64,
    /// Size and mtime of the file on this participant's disk when last seen: a touched file
    /// (same content, new mtime) updates these, never the version.
    pub local_size: u64,
    pub local_mtime: i64,
}

/// What one participant knows about a pair: every key it has, had (tombstones) or received.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Index {
    pub me: ShortId,
    pub seq: u64,
    pub entries: BTreeMap<String, IndexEntry>,
}

/// A file or folder as a scan found it. Files carry their BLAKE3; [`Index::needs_hash`] says which
/// ones must be hashed (new, or size/mtime changed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scanned {
    pub key: String,
    pub is_dir: bool,
    pub size: u64,
    pub mtime: i64,
    pub hash: Option<String>,
}

/// What a scan changed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScanChanges {
    pub changed: Vec<String>,
    /// A file that disappeared and one that appeared with the same content: (from, to).
    pub moves: Vec<(String, String)>,
}

impl Index {
    pub fn new(me: ShortId) -> Self {
        Index {
            me,
            ..Self::default()
        }
    }

    pub fn get(&self, key: &str) -> Option<&Version> {
        self.entries.get(key).map(|e| &e.version)
    }

    /// Whether a scanned file must be hashed: unknown, gone before, or size or mtime differ.
    pub fn needs_hash(&self, key: &str, size: u64, mtime: i64) -> bool {
        match self.entries.get(key) {
            Some(e) if !e.version.deleted && !e.version.is_dir => {
                e.local_size != size || e.local_mtime != mtime
            }
            _ => true,
        }
    }

    fn put(&mut self, key: &str, version: Version) {
        self.seq += 1;
        let (local_size, local_mtime) = (version.size, version.mtime);
        self.entries.insert(
            key.to_string(),
            IndexEntry {
                version,
                seq: self.seq,
                local_size,
                local_mtime,
            },
        );
    }

    /// Records a version received from another participant (or resolved with it).
    pub fn record(&mut self, key: &str, version: Version) {
        self.put(key, version);
    }

    /// Entries changed after `seq` (what a peer asks for).
    pub fn changes_since(&self, seq: u64) -> Vec<(String, Version)> {
        let mut out: Vec<(u64, String, Version)> = self
            .entries
            .iter()
            .filter(|(_, e)| e.seq > seq)
            .map(|(k, e)| (e.seq, k.clone(), e.version.clone()))
            .collect();
        out.sort_by_key(|(s, _, _)| *s);
        out.into_iter().map(|(_, k, v)| (k, v)).collect()
    }

    /// Turns the folder as scanned into versions: a new or changed item gets a version ticked by
    /// this participant, an item no longer there becomes a tombstone. A file whose content is
    /// unchanged (same hash, only the mtime moved) is updated without a new version. `now` (ms)
    /// stamps tombstones: a deletion's time is when it was seen, not the file's last mtime, so a
    /// long-untouched file deleted today keeps its tombstone for the full retention.
    pub fn apply_scan(&mut self, scan: &[Scanned], now: i64) -> ScanChanges {
        let me = self.me;
        let mut changes = ScanChanges::default();
        let mut seen: HashMap<&str, ()> = HashMap::with_capacity(scan.len());
        let mut appeared: Vec<(String, String)> = Vec::new(); // (key, hash) of new files
        for s in scan {
            seen.insert(&s.key, ());
            let old = self.entries.get(&s.key).cloned();
            let content_same = match &old {
                Some(e) if !e.version.deleted && e.version.is_dir == s.is_dir => {
                    s.is_dir
                        || (s.hash.is_none() && e.local_size == s.size && e.local_mtime == s.mtime)
                        || e.version.hash == s.hash
                }
                _ => false,
            };
            if content_same {
                if !s.is_dir {
                    // Touched, not changed: the version stays as it was; only what this disk shows moves.
                    let e = self.entries.get_mut(&s.key).expect("present");
                    e.local_size = s.size;
                    e.local_mtime = s.mtime;
                }
                continue;
            }
            let base = old.map(|e| e.version.vv).unwrap_or_default();
            let version = Version {
                is_dir: s.is_dir,
                deleted: false,
                hash: if s.is_dir { None } else { s.hash.clone() },
                size: if s.is_dir { 0 } else { s.size },
                mtime: s.mtime,
                vv: base.ticked(me),
                author: me,
            };
            if !s.is_dir
                && let Some(h) = &s.hash
            {
                appeared.push((s.key.clone(), h.clone()));
            }
            self.put(&s.key, version);
            changes.changed.push(s.key.clone());
        }
        let gone: Vec<(String, Version)> = self
            .entries
            .iter()
            .filter(|(k, e)| !e.version.deleted && !seen.contains_key(k.as_str()))
            .map(|(k, e)| (k.clone(), e.version.clone()))
            .collect();
        for (key, v) in gone {
            if !v.is_dir
                && let Some(h) = &v.hash
                && let Some(i) = appeared.iter().position(|(_, ah)| ah == h)
            {
                let (to, _) = appeared.remove(i);
                changes.moves.push((key.clone(), to));
            }
            self.put(
                &key,
                Version {
                    is_dir: v.is_dir,
                    deleted: true,
                    hash: None,
                    size: 0,
                    mtime: now.max(v.mtime),
                    vv: v.vv.ticked(me),
                    author: me,
                },
            );
            changes.changed.push(key);
        }
        changes
    }

    /// Drops tombstones whose deletion was seen before `before` (ms, the unit of every mtime here). A peer that was away longer than that may bring
    /// a deleted file back; the default keeps them 90 days (docs/DEVICE_SYNC.md §4).
    pub fn prune_tombstones(&mut self, before: i64) -> usize {
        let old: Vec<String> = self
            .entries
            .iter()
            .filter(|(_, e)| e.version.deleted && e.version.mtime < before)
            .map(|(k, _)| k.clone())
            .collect();
        for k in &old {
            self.entries.remove(k);
        }
        old.len()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ConflictMode {
    /// The losing content stays next to the file as a conflict copy.
    #[default]
    KeepBoth,
    /// The losing content goes to the bin.
    NewestWins,
}

/// What to do for one key after comparing with another participant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Step {
    /// Make the local copy match `version` (download its content, create the folder, or delete
    /// the local item to the bin), then record it.
    Take { key: String, version: Version },
    /// The content already matches: only record the merged vector, and the canonical authored
    /// time and author (the later of the two versions').
    Adopt {
        key: String,
        vv: VersionVector,
        mtime: i64,
        author: ShortId,
    },
    /// Changed on both sides and the local content lost: move the local content aside (to
    /// `copy_key`, or to the bin when `copy_key` is `None`), then take `winner`, whose vector is
    /// the merged one, and record it.
    Conflict {
        key: String,
        winner: Version,
        copy_key: Option<String>,
    },
}

/// Which of two different contents wins, the same answer on every participant.
fn wins(a: &Version, b: &Version) -> bool {
    match (a.deleted, b.deleted) {
        (false, true) => return true,
        (true, false) => return false,
        _ => {}
    }
    (a.mtime, a.author, a.hash.as_deref().unwrap_or(""), !a.is_dir)
        > (b.mtime, b.author, b.hash.as_deref().unwrap_or(""), !b.is_dir)
}

/// The equal-vector repair: content only, identical on every participant.
fn wins_by_content(a: &Version, b: &Version) -> bool {
    match (a.deleted, b.deleted) {
        (false, true) => return true,
        (true, false) => return false,
        _ => {}
    }
    (!a.is_dir, a.hash.as_deref().unwrap_or("")) > (!b.is_dir, b.hash.as_deref().unwrap_or(""))
}

/// The name a losing version keeps: `report (conflict 1a2b 9f3c07d2).txt`, after its author and its
/// content, so two conflicts the same device lost on one file get two names (#141) and every
/// participant names the same losing version the same way. Eight hex digits of the hash make two
/// different losing contents share a name about once in four billion; a name already taken here
/// gets a number on the device that makes the copy.
pub fn conflict_key(key: &str, loser: &Version) -> String {
    let (dir, name) = match key.rsplit_once('/') {
        Some((d, n)) => (Some(d), n),
        None => (None, key),
    };
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() && !loser.is_dir => (s, Some(e)),
        _ => (name, None),
    };
    let content = loser.hash.as_deref().map_or("0", |h| h.get(..8).unwrap_or(h));
    let tag = format!("{:04x} {content}", loser.author & 0xffff);
    let file = match ext {
        Some(e) => format!("{stem} (conflict {tag}).{e}"),
        None => format!("{stem} (conflict {tag})"),
    };
    match dir {
        Some(d) => format!("{d}/{file}"),
        None => file,
    }
}

/// Steps that bring `local` up to date with what `remote` reported, in key order.
pub fn plan_with(local: &Index, remote: &[(String, Version)], mode: ConflictMode) -> Vec<Step> {
    let mut steps = Vec::new();
    for (key, r) in remote {
        let Some(l) = local.get(key) else {
            steps.push(Step::Take {
                key: key.clone(),
                version: r.clone(),
            });
            continue;
        };
        match l.vv.compare(&r.vv) {
            Causality::Equal if !l.same_content(r) && !wins_by_content(l, r) => {
                let copy_key =
                    (!l.deleted && !l.is_dir && mode == ConflictMode::KeepBoth).then(|| conflict_key(key, l));
                steps.push(Step::Conflict {
                    key: key.clone(),
                    winner: r.clone(),
                    copy_key,
                });
            }
            Causality::Equal | Causality::Dominates => {}
            Causality::DominatedBy => steps.push(Step::Take {
                key: key.clone(),
                version: r.clone(),
            }),
            Causality::Concurrent => {
                let merged = l.vv.merged(&r.vv);
                if l.same_content(r) {
                    let (mtime, author) = (l.mtime, l.author).max((r.mtime, r.author));
                    steps.push(Step::Adopt {
                        key: key.clone(),
                        vv: merged,
                        mtime,
                        author,
                    });
                    continue;
                }
                if wins(l, r) {
                    // Ours wins: the side holding the other content resolves (see the module docs).
                    continue;
                }
                // Content worth keeping is a file that loses; a lost delete or folder needs no copy.
                let copy_key =
                    (!l.deleted && !l.is_dir && mode == ConflictMode::KeepBoth).then(|| conflict_key(key, l));
                steps.push(Step::Conflict {
                    key: key.clone(),
                    winner: Version {
                        vv: merged,
                        ..r.clone()
                    },
                    copy_key,
                });
            }
        }
    }
    steps
}
