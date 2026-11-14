//! Name identity: when two names are the same name (docs/adr/0004-name-identity.md).
//!
//! Windows, macOS and Android's shared storage fold case, and macOS also folds Unicode
//! normalisation (an NFD `أحمد` from a Mac and an NFC one from Drive are one file there). Drive and
//! Linux fold neither. So one pair, on every device, treats two names as the same name when their
//! [`name_key`]s are equal: NFC, then lower case. A pair holds at most one item per name key in a
//! folder; the rest of the engine keeps comparing exact keys.
//!
//! [`check_names`] applies the rule to a plan's input:
//! - the same file spelled differently on Drive and on the device is one key (Drive's spelling),
//!   whether it met for the first time (NFC on Drive, NFD on a Mac) or Drive renamed it;
//! - a name held twice on one side (two spellings in one Drive folder, or on a case-sensitive
//!   device) is a clash: every key with that name, and everything under it, stays out of the plan
//!   and is reported, never merged or deleted (SRS FR-C7);
//! - a name the device renamed (case only, say) stays two keys, so the rename reaches Drive as an
//!   upload and the old name's removal, as any other rename made on the device.

use std::collections::{BTreeMap, BTreeSet};

use unicode_normalization::UnicodeNormalization;

use crate::model::{BaselineEntry, PlanInput, RemoteEntry};

/// The form two names are compared in: Unicode NFC, then lower case (and NFC again, since lower
/// casing can denormalise). Applied to a whole `/`-separated path, it compares each component.
pub fn name_key(name: &str) -> String {
    let nfc: String = name.nfc().collect();
    nfc.to_lowercase().nfc().collect()
}

/// True when `a` and `b` are the same name under [`name_key`].
pub fn same_name(a: &str, b: &str) -> bool {
    a == b || name_key(a) == name_key(b)
}

/// What [`check_names`] did to a plan's input.
#[derive(Debug, Default)]
pub struct NameCheck {
    pub input: PlanInput,
    /// Baseline rows moved to Drive's spelling of their name: (old key, new row). The engine
    /// stores them, so the next run starts from the new key.
    pub rekeyed: Vec<(String, BaselineEntry)>,
    /// Paths whose name is held twice on one side, left out of this plan.
    pub clashes: Vec<String>,
}

fn is_under(key: &str, folder: &str) -> bool {
    key.len() > folder.len() && key.starts_with(folder) && key.as_bytes()[folder.len()] == b'/'
}

/// Takes Drive's id for a synced row when Drive's copy has the content the row describes, so the
/// planner moves nothing. Returns false (and changes nothing) when the content differs.
pub fn adopt_drive_identity(row: &mut BaselineEntry, r: &RemoteEntry) -> bool {
    let same = r.is_dir == row.is_dir
        && (r.is_dir || (r.md5.is_some() && r.md5 == row.remote_md5 && r.size == row.remote_size));
    if same {
        row.remote_id = Some(r.id.clone());
        row.remote_version = r.version;
        row.remote_mtime = Some(r.mtime);
    }
    same
}

/// True when `key` is `folder` or inside it, comparing name keys.
pub fn same_name_or_under(key: &str, folder: &str) -> bool {
    let (k, f) = (name_key(key), name_key(folder));
    k == f || is_under(&k, &f)
}

fn in_clash(clash: &BTreeSet<String>, k: &str) -> bool {
    let nk = name_key(k);
    clash.contains(&nk) || clash.iter().any(|c| is_under(&nk, c))
}

/// Moves `from`'s local entry to `to`, keeping its local name.
fn move_local(input: &mut PlanInput, from: &str, to: &str) {
    if let Some(l) = input.local.remove(from) {
        input.local.insert(to.to_owned(), l);
    }
    if let Some(rel) = input.local_rel_by_key.remove(from) {
        input.local_rel_by_key.insert(to.to_owned(), rel);
    }
}

/// Drive's one key per name key; `None` when Drive has several (a clash).
fn drive_keys_by_name(input: &PlanInput) -> BTreeMap<String, Option<String>> {
    let mut out: BTreeMap<String, Option<String>> = BTreeMap::new();
    for k in input.remote.keys() {
        out.entry(name_key(k))
            .and_modify(|v| *v = None)
            .or_insert_with(|| Some(k.clone()));
    }
    out
}

/// Pass 1. Drive spells a synced name differently now (renamed on Drive, or re-created by a device
/// that renamed it): the baseline row follows Drive's spelling and the device keeps its own, so
/// nothing reads as deleted on either side.
fn follow_drive_spelling(input: &mut PlanInput, clash: &BTreeSet<String>) -> Vec<(String, BaselineEntry)> {
    let by_name = drive_keys_by_name(input);
    let olds: Vec<String> = input
        .baseline
        .keys()
        .filter(|k| !input.remote.contains_key(*k) && !in_clash(clash, k))
        .cloned()
        .collect();
    let mut rekeyed = Vec::new();
    for old in olds {
        let Some(Some(new)) = by_name.get(&name_key(&old)).cloned() else {
            continue;
        };
        let here_as_new = input.local.contains_key(&new);
        if input.baseline.contains_key(&new) || (here_as_new && input.local.contains_key(&old)) {
            continue;
        }
        let Some(mut row) = input.baseline.remove(&old) else {
            continue;
        };
        adopt_drive_identity(&mut row, &input.remote[&new]);
        row.key = new.clone();
        if here_as_new {
            // The device already spells it Drive's way (a folder whose files were renamed).
            row.local_rel = input.local[&new].rel.clone();
            input.local_rel_by_key.remove(&old);
        } else {
            move_local(input, &old, &new);
        }
        input.baseline.insert(new, row.clone());
        rekeyed.push((old, row));
    }
    rekeyed
}

/// Pass 2. First meeting: a device name nothing knows yet whose Drive spelling differs (NFD here,
/// NFC on Drive; another case) is that Drive key, kept under the device's own name.
fn merge_first_meetings(input: &mut PlanInput) {
    let by_name = drive_keys_by_name(input);
    let fresh: Vec<String> = input
        .local
        .keys()
        .filter(|k| !input.remote.contains_key(*k) && !input.baseline.contains_key(*k))
        .cloned()
        .collect();
    for key in fresh {
        let Some(Some(drive)) = by_name.get(&name_key(&key)).cloned() else {
            continue;
        };
        if !input.local.contains_key(&drive) && !input.baseline.contains_key(&drive) {
            move_local(input, &key, &drive);
        }
    }
}

/// Pass 3. Items new on the device inside a folder whose key differs from its local name (a folder
/// Drive respelled, a first meeting) take the folder's key, so they upload into it.
fn follow_folder_keys(input: &mut PlanInput) {
    let folders: BTreeMap<String, String> = input
        .local
        .iter()
        .filter(|(k, l)| l.is_dir && l.rel != **k)
        .map(|(k, l)| (l.rel.clone(), k.clone()))
        .collect();
    if folders.is_empty() {
        return;
    }
    let fresh: Vec<(String, String)> = input
        .local
        .iter()
        .filter(|(k, _)| !input.remote.contains_key(*k) && !input.baseline.contains_key(*k))
        .map(|(k, l)| (k.clone(), l.rel.clone()))
        .collect();
    for (key, rel) in fresh {
        // The deepest folder above `rel` with a key of its own.
        let mut at = rel.as_str();
        let mut to = None;
        while let Some((parent, _)) = at.rsplit_once('/') {
            if let Some(folder_key) = folders.get(parent) {
                to = Some(format!("{folder_key}{}", &rel[parent.len()..]));
                break;
            }
            at = parent;
        }
        if let Some(to) = to.filter(|t| *t != key && !input.local.contains_key(t)) {
            move_local(input, &key, &to);
        }
    }
}

/// Pass 4. What is left: one name, two or more keys. A rename made on the device (the old name
/// synced and gone here, the new name only here) syncs as a rename when Drive's copy is as synced
/// or gone. When Drive changed it meanwhile, the device's file stays that key under its new name,
/// so the usual conflict rules apply. Anything else holds the name twice on one side: a clash.
fn find_clashes(input: &mut PlanInput, clash: &mut BTreeSet<String>) {
    let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut all: BTreeSet<&String> = BTreeSet::new();
    all.extend(input.baseline.keys());
    all.extend(input.local.keys());
    all.extend(input.remote.keys());
    for k in all {
        groups.entry(name_key(k)).or_default().push(k.clone());
    }
    for (nk, keys) in groups {
        if keys.len() < 2 || clash.contains(&nk) {
            continue;
        }
        let renamed = |old: &str, new: &str| {
            input.baseline.contains_key(old)
                && !input.local.contains_key(old)
                && input.local.contains_key(new)
                && !input.remote.contains_key(new)
                && !input.baseline.contains_key(new)
        };
        let rename = match keys.as_slice() {
            [a, b] if renamed(a, b) => Some((a.clone(), b.clone())),
            [a, b] if renamed(b, a) => Some((b.clone(), a.clone())),
            _ => None,
        };
        let Some((old, new)) = rename else {
            clash.insert(nk);
            continue;
        };
        let changed_on_drive = match (&input.baseline[&old], input.remote.get(&old)) {
            (b, Some(r)) => !(r.is_dir && b.is_dir) && !crate::planner::remote_unchanged_by_identity(r, b),
            (_, None) => false,
        };
        if changed_on_drive {
            move_local(input, &new, &old);
        }
    }
}

/// Applies the name rule to `input`. `drive_clashes` are the Drive paths the listing left out
/// because another item in the same folder has the same name.
pub fn check_names(mut input: PlanInput, drive_clashes: &[String]) -> NameCheck {
    let mut clash: BTreeSet<String> = drive_clashes.iter().map(|k| name_key(k)).collect();
    let rekeyed = follow_drive_spelling(&mut input, &clash);
    merge_first_meetings(&mut input);
    follow_folder_keys(&mut input);
    find_clashes(&mut input, &mut clash);

    // A synced item Drive moved into a clashing folder is not deleted on Drive: it waits too.
    let ids_in_clash: BTreeSet<&str> = input
        .remote
        .iter()
        .filter(|(k, _)| in_clash(&clash, k))
        .map(|(_, r)| r.id.as_str())
        .collect();
    let moved_into_clash: BTreeSet<String> = input
        .baseline
        .iter()
        .filter(|(k, b)| {
            !input.remote.contains_key(*k)
                && b.remote_id.as_deref().is_some_and(|id| ids_in_clash.contains(id))
        })
        .map(|(k, _)| k.clone())
        .collect();

    let mut clashes: BTreeSet<String> = BTreeSet::new();
    let input = input.without_ignored(|k, _| {
        let hit = in_clash(&clash, k) || moved_into_clash.contains(k);
        if hit {
            clashes.insert(k.to_owned());
        }
        hit
    });
    // Report each spelling of a clashing name, not everything under a clashing folder.
    let mut reported: Vec<String> = Vec::new();
    for k in clashes.into_iter().chain(drive_clashes.iter().cloned()) {
        if !reported
            .iter()
            .any(|r| r == &k || is_under(&name_key(&k), &name_key(r)))
        {
            reported.push(k);
        }
    }
    NameCheck {
        input,
        rekeyed,
        clashes: reported,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{LocalEntry, RemoteEntry};

    fn remote(key: &str, id: &str, md5: &str) -> RemoteEntry {
        RemoteEntry {
            key: key.into(),
            id: id.into(),
            name: key.rsplit('/').next().unwrap_or(key).into(),
            is_dir: false,
            size: Some(1),
            md5: Some(md5.into()),
            mtime: 1,
            version: Some(1),
            google_mime: None,
            parent_id: None,
        }
    }
    fn local(rel: &str) -> LocalEntry {
        LocalEntry {
            rel: rel.into(),
            is_dir: false,
            size: 1,
            mtime: 1,
        }
    }
    fn synced(key: &str, id: &str, md5: &str) -> BaselineEntry {
        BaselineEntry {
            key: key.into(),
            local_rel: key.into(),
            is_dir: false,
            local_size: 1,
            local_mtime: 1,
            local_hash: None,
            remote_id: Some(id.into()),
            remote_md5: Some(md5.into()),
            remote_size: Some(1),
            remote_mtime: Some(1),
            remote_version: Some(1),
            export_mime: None,
        }
    }

    #[test]
    fn case_and_normalisation_fold_to_one_key() {
        assert_eq!(name_key("Foo.TXT"), name_key("foo.txt"));
        assert_eq!(name_key("caf\u{e9}"), name_key("cafe\u{301}"));
        assert_eq!(name_key("\u{623}\u{62d}"), name_key("\u{627}\u{654}\u{62d}"));
        assert_ne!(name_key("foo.txt"), name_key("fo.txt"));
        assert!(same_name("A/B.txt", "a/b.TXT"));
    }

    #[test]
    fn a_name_drive_respelled_moves_the_baseline_and_keeps_the_device_name() {
        let mut input = PlanInput::default();
        input
            .baseline
            .insert("Foo.txt".into(), synced("Foo.txt", "1", "m"));
        input.local.insert("Foo.txt".into(), local("Foo.txt"));
        input.local_rel_by_key.insert("Foo.txt".into(), "Foo.txt".into());
        input.remote.insert("foo.txt".into(), remote("foo.txt", "2", "m"));
        let c = check_names(input, &[]);
        assert!(c.clashes.is_empty());
        assert_eq!(vec!["foo.txt"], c.input.baseline.keys().collect::<Vec<_>>());
        assert_eq!(
            Some(&"Foo.txt".to_owned()),
            c.input.local_rel_by_key.get("foo.txt")
        );
        assert_eq!(
            Some("2"),
            c.input.baseline["foo.txt"].remote_id.as_deref(),
            "same content: Drive's id"
        );
        assert_eq!(1, c.rekeyed.len());
    }

    #[test]
    fn a_name_held_twice_on_one_side_stays_out_of_the_plan() {
        let mut input = PlanInput::default();
        input.local.insert("Notes.txt".into(), local("Notes.txt"));
        input.local.insert("notes.txt".into(), local("notes.txt"));
        input.local.insert("other.txt".into(), local("other.txt"));
        let c = check_names(input, &[]);
        assert_eq!(vec!["other.txt"], c.input.local.keys().collect::<Vec<_>>());
        assert_eq!(2, c.clashes.len());
    }

    #[test]
    fn a_drive_clash_covers_both_spellings_and_what_is_under_them() {
        let mut input = PlanInput::default();
        input.baseline.insert(
            "Docs".into(),
            BaselineEntry {
                is_dir: true,
                ..synced("Docs", "1", "")
            },
        );
        input
            .baseline
            .insert("Docs/a.txt".into(), synced("Docs/a.txt", "3", "m"));
        input.local.insert("Docs/a.txt".into(), local("Docs/a.txt"));
        input.remote.insert(
            "docs".into(),
            RemoteEntry {
                is_dir: true,
                md5: None,
                ..remote("docs", "2", "")
            },
        );
        let c = check_names(input, &["Docs".to_owned()]);
        assert!(c.input.baseline.is_empty() && c.input.local.is_empty() && c.input.remote.is_empty());
        assert_eq!(vec!["Docs".to_owned(), "docs".to_owned()], c.clashes);
    }

    #[test]
    fn a_rename_made_on_the_device_stays_a_rename() {
        let mut input = PlanInput::default();
        input
            .baseline
            .insert("meeting.txt".into(), synced("meeting.txt", "1", "m"));
        input
            .remote
            .insert("meeting.txt".into(), remote("meeting.txt", "1", "m"));
        input.local.insert("Meeting.txt".into(), local("Meeting.txt"));
        let c = check_names(input.clone(), &[]);
        assert!(c.clashes.is_empty());
        assert_eq!(input, c.input);
    }
}
