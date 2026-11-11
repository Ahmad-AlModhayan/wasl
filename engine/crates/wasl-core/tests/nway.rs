//! The N-way model on simulated devices (docs/DEVICE_SYNC.md §4): folders as maps, a scan that
//! turns them into versions, and a small executor applying [`Step`]s. Property: any edits on 3
//! devices, synced in any order, end in one state on all of them.

use proptest::prelude::*;
use std::collections::BTreeMap;
use wasl_core::nway::{ConflictMode, Index, Scanned, Step, Version, plan_with};

#[derive(Debug, Clone, PartialEq, Eq)]
enum Node {
    Dir,
    /// Content (stands in for bytes; its "hash" is the content itself) and mtime.
    File(String, i64),
}

#[derive(Debug, Clone)]
struct Device {
    idx: Index,
    fs: BTreeMap<String, Node>,
    clock: i64,
    /// Versions the person here replaced on purpose (edited or deleted the file while the index
    /// held that version): (key, vector they built on).
    replaced: Vec<(String, wasl_core::vv::VersionVector)>,
}

impl Device {
    fn new(id: u64) -> Self {
        Device {
            idx: Index::new(id),
            fs: BTreeMap::new(),
            clock: 1_000 * id as i64,
            replaced: Vec::new(),
        }
    }

    fn tick(&mut self) -> i64 {
        self.clock += 7;
        self.clock
    }

    fn ensure_parents(&mut self, key: &str) {
        let mut acc = String::new();
        let parts: Vec<&str> = key.split('/').collect();
        for p in &parts[..parts.len() - 1] {
            if !acc.is_empty() {
                acc.push('/');
            }
            acc.push_str(p);
            self.fs.insert(acc.clone(), Node::Dir);
        }
    }

    fn write(&mut self, key: &str, content: &str) {
        // A folder cannot be overwritten by a file in place; editing a folder's key is skipped.
        if matches!(self.fs.get(key), Some(Node::Dir)) {
            return;
        }
        if self.fs.keys().any(|k| k.starts_with(&format!("{key}/"))) {
            return;
        }
        self.ensure_parents(key);
        let t = self.tick();
        self.fs.insert(key.into(), Node::File(content.into(), t));
    }

    fn delete(&mut self, key: &str) {
        if matches!(self.fs.get(key), Some(Node::File(..))) {
            self.fs.remove(key);
        }
    }

    fn scan(&mut self) {
        for (k, e) in &self.idx.entries {
            if e.version.deleted || e.version.is_dir {
                continue;
            }
            let same =
                matches!(self.fs.get(k), Some(Node::File(c, _)) if e.version.hash.as_deref() == Some(c));
            if !same {
                self.replaced.push((k.clone(), e.version.vv.clone()));
            }
        }
        let scan: Vec<Scanned> = self
            .fs
            .iter()
            .map(|(k, n)| match n {
                Node::Dir => Scanned {
                    key: k.clone(),
                    is_dir: true,
                    size: 0,
                    mtime: 0,
                    hash: None,
                },
                Node::File(c, t) => Scanned {
                    key: k.clone(),
                    is_dir: false,
                    size: c.len() as u64,
                    mtime: *t,
                    hash: Some(c.clone()),
                },
            })
            .collect();
        self.idx.apply_scan(&scan, self.clock);
    }

    fn materialise(&mut self, key: &str, v: &Version) {
        if v.deleted {
            match self.fs.get(key) {
                Some(Node::File(..)) => {
                    self.fs.remove(key);
                }
                Some(Node::Dir) if !self.fs.keys().any(|k| k.starts_with(&format!("{key}/"))) => {
                    self.fs.remove(key);
                }
                _ => {}
            }
        } else if v.is_dir {
            if !matches!(self.fs.get(key), Some(Node::File(..))) {
                self.ensure_parents(key);
                self.fs.insert(key.into(), Node::Dir);
            }
        } else if !matches!(self.fs.get(key), Some(Node::Dir)) {
            self.ensure_parents(key);
            self.fs
                .insert(key.into(), Node::File(v.hash.clone().unwrap(), v.mtime));
        }
    }

    /// A local file the index does not know as it is (written since the last scan). Like the real
    /// executor, the simulator never overwrites one: the step waits for the next scan and sync.
    fn dirty(&self, key: &str) -> bool {
        match self.fs.get(key) {
            Some(Node::File(c, _)) => self
                .idx
                .get(key)
                .is_none_or(|v| v.deleted || v.hash.as_deref() != Some(c)),
            _ => false,
        }
    }

    fn unique(&self, key: &str) -> String {
        let mut k = key.to_string();
        let mut n = 2;
        while self.fs.contains_key(&k) {
            k = format!("{key} {n}");
            n += 1;
        }
        k
    }

    /// Pulls everything `other` knows and applies it; returns how many steps there were.
    fn pull(&mut self, other: &Device, mode: ConflictMode) -> usize {
        let remote = other.idx.changes_since(0);
        let steps = plan_with(&self.idx, &remote, mode);
        let n = steps.len();
        for s in steps {
            match s {
                Step::Take { key, version } => {
                    if self.dirty(&key) {
                        continue;
                    }
                    self.materialise(&key, &version);
                    self.idx.record(&key, version);
                }
                Step::Adopt {
                    key,
                    vv,
                    mtime,
                    author,
                } => {
                    let mut v = self.idx.get(&key).unwrap().clone();
                    v.vv = vv;
                    v.mtime = mtime;
                    v.author = author;
                    self.idx.record(&key, v);
                }
                Step::Conflict {
                    key,
                    winner,
                    copy_key,
                } => {
                    if self.dirty(&key) {
                        continue;
                    }
                    if let (Some(copy), Some(Node::File(c, t))) = (copy_key, self.fs.get(&key).cloned()) {
                        let copy = self.unique(&copy);
                        self.fs.insert(copy, Node::File(c, t));
                    }
                    if matches!(self.fs.get(&key), Some(Node::File(..))) {
                        self.fs.remove(&key);
                    }
                    self.materialise(&key, &winner);
                    self.idx.record(&key, winner);
                }
            }
        }
        self.scan();
        n
    }
}

/// Syncs every device with every other until nothing changes (at most 12 rounds).
fn settle(devs: &mut [Device], mode: ConflictMode) {
    for _ in 0..12 {
        let mut moved = 0;
        for i in 0..devs.len() {
            for j in 0..devs.len() {
                if i != j {
                    let other = devs[j].clone();
                    moved += devs[i].pull(&other, mode);
                }
            }
        }
        if moved == 0 {
            return;
        }
    }
    panic!("did not settle in 12 rounds");
}

fn live(d: &Device) -> BTreeMap<String, (bool, Option<String>)> {
    d.idx
        .entries
        .iter()
        .filter(|(_, e)| !e.version.deleted)
        .map(|(k, e)| (k.clone(), (e.version.is_dir, e.version.hash.clone())))
        .collect()
}

fn contents(d: &Device) -> Vec<String> {
    d.fs.values()
        .filter_map(|n| match n {
            Node::File(c, _) => Some(c.clone()),
            Node::Dir => None,
        })
        .collect()
}

fn assert_converged(devs: &[Device]) {
    for d in &devs[1..] {
        assert_eq!(
            devs[0].fs.keys().collect::<Vec<_>>(),
            d.fs.keys().collect::<Vec<_>>(),
            "same names"
        );
        assert_eq!(contents(&devs[0]), contents(d), "same contents");
        assert_eq!(live(&devs[0]), live(d), "same index");
    }
}

#[test]
fn an_edit_travels_and_a_stale_device_never_undoes_a_delete() {
    let mut d = vec![Device::new(1), Device::new(2), Device::new(3)];
    d[0].write("a.txt", "v1");
    d[0].scan();
    settle(&mut d, ConflictMode::KeepBoth);
    assert!(d.iter().all(|x| x.fs.get("a.txt") == d[0].fs.get("a.txt")));

    // Device 1 deletes; device 3 is offline. When 3 comes back it must not bring a.txt back.
    d[0].delete("a.txt");
    d[0].scan();
    let one = d[0].clone();
    d[1].pull(&one, ConflictMode::KeepBoth);
    let two = d[1].clone();
    let stale = d[2].clone();
    d[0].pull(&stale, ConflictMode::KeepBoth); // stale peer first
    d[2].pull(&two, ConflictMode::KeepBoth);
    settle(&mut d, ConflictMode::KeepBoth);
    assert!(d.iter().all(|x| !x.fs.contains_key("a.txt")), "{:?}", d[2].fs);
}

#[test]
fn concurrent_different_edits_keep_both_contents() {
    let mut d = vec![Device::new(1), Device::new(2)];
    d[0].write("r.txt", "base");
    d[0].scan();
    settle(&mut d, ConflictMode::KeepBoth);
    d[0].write("r.txt", "from one");
    d[0].scan();
    d[1].write("r.txt", "from two");
    d[1].scan();
    settle(&mut d, ConflictMode::KeepBoth);
    assert_converged(&d);
    let all = contents(&d[0]);
    assert!(
        all.contains(&"from one".into()) && all.contains(&"from two".into()),
        "{all:?}"
    );
    assert_eq!(2, d[0].fs.len(), "one file and one conflict copy: {:?}", d[0].fs);
}

#[test]
fn the_same_edit_on_two_devices_is_not_a_conflict() {
    let mut d = vec![Device::new(1), Device::new(2)];
    d[0].write("s.txt", "same");
    d[0].scan();
    d[1].write("s.txt", "same");
    d[1].scan();
    settle(&mut d, ConflictMode::KeepBoth);
    assert_converged(&d);
    assert_eq!(1, d[0].fs.len(), "no conflict copy: {:?}", d[0].fs);
}

#[test]
fn a_change_beats_a_concurrent_delete() {
    let mut d = vec![Device::new(1), Device::new(2)];
    d[0].write("k.txt", "base");
    d[0].scan();
    settle(&mut d, ConflictMode::KeepBoth);
    d[0].delete("k.txt");
    d[0].scan();
    d[1].write("k.txt", "edited");
    d[1].scan();
    settle(&mut d, ConflictMode::KeepBoth);
    assert_converged(&d);
    assert_eq!(
        Some(&Node::File(
            "edited".into(),
            d[1].fs
                .get("k.txt")
                .map(|n| match n {
                    Node::File(_, t) => *t,
                    _ => 0,
                })
                .unwrap()
        )),
        d[0].fs.get("k.txt")
    );
}

#[test]
fn newest_wins_mode_resolves_without_a_copy() {
    let mut d = vec![Device::new(1), Device::new(2)];
    d[0].write("n.txt", "base");
    d[0].scan();
    settle(&mut d, ConflictMode::NewestWins);
    d[0].write("n.txt", "older");
    d[0].scan();
    d[1].clock += 1_000_000;
    d[1].write("n.txt", "newer");
    d[1].scan();
    settle(&mut d, ConflictMode::NewestWins);
    assert_converged(&d);
    assert_eq!(vec!["newer".to_string()], contents(&d[0]));
}

#[test]
fn a_move_is_detected_by_content() {
    let mut idx = Index::new(1);
    let f = |k: &str| Scanned {
        key: k.into(),
        is_dir: false,
        size: 3,
        mtime: 5,
        hash: Some("h1".into()),
    };
    idx.apply_scan(&[f("old/name.txt")], 0);
    let ch = idx.apply_scan(&[f("new/name.txt")], 0);
    assert_eq!(
        vec![("old/name.txt".to_string(), "new/name.txt".to_string())],
        ch.moves
    );
}

#[test]
fn a_touch_without_a_content_change_is_not_a_new_version() {
    let mut idx = Index::new(1);
    let s = |t| Scanned {
        key: "t.txt".into(),
        is_dir: false,
        size: 3,
        mtime: t,
        hash: Some("h".into()),
    };
    idx.apply_scan(&[s(5)], 0);
    let vv = idx.get("t.txt").unwrap().vv.clone();
    let ch = idx.apply_scan(&[s(9)], 0);
    assert!(ch.changed.is_empty());
    assert_eq!(vv, idx.get("t.txt").unwrap().vv);
    assert_eq!(
        5,
        idx.get("t.txt").unwrap().mtime,
        "the version keeps its own mtime"
    );
    assert_eq!(
        9, idx.entries["t.txt"].local_mtime,
        "what the disk shows is noted apart"
    );
    assert!(!idx.needs_hash("t.txt", 3, 9));
}

#[derive(Debug, Clone)]
enum Op {
    Write(usize, usize, usize),
    Delete(usize, usize),
    Scan(usize),
    Pull(usize, usize),
}

const KEYS: [&str; 5] = ["a.txt", "b.txt", "d/c.txt", "d/e/f.txt", "g"];
const CONTENTS: [&str; 4] = ["one", "two", "three", "four"];

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        3 => (0..3usize, 0..KEYS.len(), 0..CONTENTS.len()).prop_map(|(d, k, c)| Op::Write(d, k, c)),
        1 => (0..3usize, 0..KEYS.len()).prop_map(|(d, k)| Op::Delete(d, k)),
        2 => (0..3usize).prop_map(Op::Scan),
        3 => (0..3usize, 0..3usize).prop_map(|(a, b)| Op::Pull(a, b)),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(400))]

    /// Any history on 3 devices, synced in any order, converges once everyone has met everyone.
    #[test]
    fn three_devices_converge(ops in proptest::collection::vec(op(), 1..60), newest in any::<bool>()) {
        let mode = if newest { ConflictMode::NewestWins } else { ConflictMode::KeepBoth };
        let mut d = vec![Device::new(0x11), Device::new(0x22), Device::new(0x33)];
        for o in ops {
            match o {
                Op::Write(i, k, c) => d[i].write(KEYS[k], CONTENTS[c]),
                Op::Delete(i, k) => d[i].delete(KEYS[k]),
                Op::Scan(i) => d[i].scan(),
                Op::Pull(a, b) if a != b => {
                    d[a].scan();
                    let other = d[b].clone();
                    d[a].pull(&other, mode);
                }
                Op::Pull(..) => {}
            }
        }
        for x in d.iter_mut() {
            x.scan();
        }
        settle(&mut d, mode);
        assert_converged(&d);
    }

    /// Content written last on some device before everyone met survives somewhere (KeepBoth).
    #[test]
    fn keep_both_loses_no_final_content(ops in proptest::collection::vec(op(), 1..40)) {
        let mut d = vec![Device::new(0x11), Device::new(0x22), Device::new(0x33)];
        for o in ops {
            match o {
                Op::Write(i, k, c) => d[i].write(KEYS[k], CONTENTS[c]),
                Op::Delete(i, k) => d[i].delete(KEYS[k]),
                Op::Scan(i) => d[i].scan(),
                Op::Pull(a, b) if a != b => {
                    d[a].scan();
                    let other = d[b].clone();
                    d[a].pull(&other, ConflictMode::KeepBoth);
                }
                Op::Pull(..) => {}
            }
        }
        for x in d.iter_mut() {
            x.scan();
        }
        // Every file content a device holds now must exist after settling, unless someone
        // deliberately edited or deleted that key after having seen the content.
        // A content's origin is its author's tick ("dot"): (author, counter).
        let before: Vec<(String, String, (u64, u64))> = d
            .iter()
            .flat_map(|x| {
                x.fs.iter()
                    .filter_map(|(k, n)| match n {
                        Node::File(c, _) => {
                            let v = x.idx.get(k).unwrap();
                            Some((k.clone(), c.clone(), (v.author, v.vv.get(v.author))))
                        }
                        Node::Dir => None,
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        settle(&mut d, ConflictMode::KeepBoth);
        let after = contents(&d[0]);
        for (k, c, (author, n)) in before {
            // Replaced on purpose by someone who had already seen this content's origin.
            let replaced = d
                .iter()
                .flat_map(|x| x.replaced.iter())
                .any(|(rk, base)| *rk == k && base.get(author) >= n);
            prop_assert!(replaced || after.contains(&c), "lost {c:?} at {k}");
        }
    }
}

#[test]
fn a_tombstone_ages_from_when_the_delete_was_seen() {
    let mut idx = Index::new(1);
    let old_file = Scanned {
        key: "old.txt".into(),
        is_dir: false,
        size: 1,
        mtime: 1_000,
        hash: Some("h".into()),
    };
    idx.apply_scan(&[old_file], 1_000);
    let day = 86_400_000;
    let deleted_at = 400 * day;
    idx.apply_scan(&[], deleted_at);
    // Pruning everything older than a day before the delete keeps it, though the file itself is
    // 400 days old.
    assert_eq!(
        0,
        idx.prune_tombstones(deleted_at - day),
        "a fresh tombstone stays"
    );
    assert!(idx.get("old.txt").unwrap().deleted);
    assert_eq!(1, idx.prune_tombstones(deleted_at + 1));
}

#[test]
fn two_conflicts_lost_by_one_device_get_two_copy_names() {
    // #141 H3: the copy name followed the author only.
    let lost = |hash: &str| Version {
        is_dir: false,
        deleted: false,
        hash: Some(hash.to_owned()),
        size: 1,
        mtime: 1,
        vv: Default::default(),
        author: 7,
    };
    let one = wasl_core::nway::conflict_key("d/f.txt", &lost("aa11bb22cc33"));
    let two = wasl_core::nway::conflict_key("d/f.txt", &lost("aa11bb23cc33"));
    assert_eq!("d/f (conflict 0007 aa11bb22).txt", one);
    assert_ne!(one, two);
}
