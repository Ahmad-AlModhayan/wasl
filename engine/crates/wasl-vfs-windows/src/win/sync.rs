//! Keeping the placeholders in step with the pair's [`Tree`]: what is on disk is compared with
//! the tree, and only Wasl's own in-sync placeholders are changed or removed. A file changed here
//! (not in sync), a local-only file, or a name Windows refuses is left alone and reported; the
//! engine's sync decides about those as for any other pair (docs/ON_DEMAND.md §2.3).

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use wasl_vfs::{Kept, KeptKind, Node, Tree};

use super::cf::{self, CF_PIN_STATE_PINNED, CF_PIN_STATE_UNSPECIFIED, NewPlaceholder};
use super::{FILE_ATTRIBUTE_PINNED, OnDemandRoot, os, walk};
use crate::identity::{self, Identity};
use crate::{Error, Result, SyncReport};

/// One thing on disk, with its placeholder state.
struct OnDisk {
    path: PathBuf,
    is_dir: bool,
    size: u64,
    mtime_ms: i64,
    readonly: bool,
    placeholder: Option<cf::Info>,
}

impl OnDemandRoot {
    /// Creates, updates and removes placeholders so the folder shows `tree`.
    pub fn sync(&self, tree: &Tree) -> Result<SyncReport> {
        self.shared.know(tree, true);
        let mut report = SyncReport::default();
        let mut disk: BTreeMap<String, OnDisk> = BTreeMap::new();
        for e in walk(self.root()) {
            let Ok(meta) = fs::symlink_metadata(&e.path) else {
                continue;
            };
            let placeholder = cf::open(&e.path, false)
                .and_then(|f| cf::info(&f))
                .unwrap_or(None);
            disk.insert(
                e.rel,
                OnDisk {
                    path: e.path,
                    is_dir: e.is_dir,
                    size: meta.len(),
                    mtime_ms: mtime_ms(&meta),
                    readonly: meta.permissions().readonly(),
                    placeholder,
                },
            );
        }

        // Removals, children before their folders.
        let mut gone: HashSet<&str> = HashSet::new();
        for (rel, d) in disk.iter().rev() {
            if tree.get(rel).is_some_and(|n| n.is_dir == d.is_dir) {
                continue;
            }
            let kept = |kind, why: &str| Kept::new(rel.clone(), kind, why);
            match &d.placeholder {
                None => report
                    .kept
                    .push(kept(KeptKind::OnlyHere, "only here: not in the listing")),
                Some(ph) if !d.is_dir && !ph.in_sync => report
                    .kept
                    .push(kept(KeptKind::ChangedHere, "changed here and not uploaded yet")),
                Some(ph) => {
                    let r = if d.is_dir {
                        fs::remove_dir(&d.path)
                    } else {
                        fs::remove_file(&d.path)
                    };
                    match r {
                        Ok(()) => {
                            if let Some(Identity::File(f)) = identity::decode(&ph.identity) {
                                self.shared.hydrator.cache().forget(&f);
                            }
                            gone.insert(rel);
                            report.removed += 1;
                        }
                        Err(e) => report
                            .kept
                            .push(kept(KeptKind::Failed, &format!("could not remove: {e}"))),
                    }
                }
            }
        }

        // Creations (grouped by folder, folders first) and updates.
        let mut create: BTreeMap<&str, Vec<&Node>> = BTreeMap::new();
        for n in tree.iter() {
            match disk.get(&n.path) {
                Some(d) if !gone.contains(n.path.as_str()) => {
                    if !n.is_dir && !d.is_dir {
                        self.update(n, d, &mut report);
                    }
                }
                _ => create.entry(n.parent()).or_default().push(n),
            }
        }
        for (parent, nodes) in create {
            let dir = self.root().join(parent.replace('/', "\\"));
            let mut entries = Vec::new();
            let mut paths = Vec::new();
            for n in nodes {
                let id = match &n.file {
                    Some(f) if !n.is_dir => identity::encode_file(f),
                    _ => Some(identity::encode_dir()),
                };
                let Some(id) = id else {
                    report.kept.push(Kept::new(
                        &n.path,
                        KeptKind::Unsupported,
                        "its id is too long for a placeholder",
                    ));
                    continue;
                };
                entries.push(NewPlaceholder {
                    name: n.name().to_owned(),
                    is_dir: n.is_dir,
                    size: n.size(),
                    mtime_ms: n.mtime_ms,
                    identity: id,
                    read_only: self.shared.read_only,
                });
                paths.push(n);
            }
            let pinned_parent =
                fs::symlink_metadata(&dir).is_ok_and(|m| m.file_attributes() & FILE_ATTRIBUTE_PINNED != 0);
            for (n, r) in paths.into_iter().zip(cf::create(&dir, &entries)) {
                match r {
                    Ok(()) => {
                        report.created += 1;
                        if pinned_parent {
                            // A new file in an "always keep" folder is kept too.
                            let p = dir.join(n.name());
                            if let Ok(f) = cf::open(&p, true) {
                                let _ = cf::set_pin(&f, CF_PIN_STATE_PINNED, n.is_dir);
                            }
                            if let Some(w) = &self.watcher {
                                w.poke(p);
                            }
                        }
                    }
                    Err(e) => report.kept.push(Kept::new(
                        &n.path,
                        KeptKind::Failed,
                        format!("could not create: {e}"),
                    )),
                }
            }
        }
        self.shared.know(tree, false);
        Ok(report)
    }

    /// A file both on disk and in the tree.
    fn update(&self, n: &Node, d: &OnDisk, report: &mut SyncReport) {
        let Some(want) = &n.file else { return };
        let kept =
            |report: &mut SyncReport, kind, why: String| report.kept.push(Kept::new(&n.path, kind, why));
        let Some(blob) = identity::encode_file(want) else {
            return kept(
                report,
                KeptKind::Unsupported,
                "its id is too long for a placeholder".into(),
            );
        };
        let Some(ph) = &d.placeholder else {
            // A regular file: the same size and time as the listing make it the listed
            // version (as the engine's baseline would); it becomes a full placeholder.
            if d.size == want.size && d.mtime_ms == n.mtime_ms {
                match cf::open(&d.path, true).and_then(|f| cf::convert(&f, &blob)) {
                    Ok(()) => report.converted += 1,
                    Err(e) => kept(report, KeptKind::Failed, format!("could not convert: {e}")),
                }
            } else {
                kept(
                    report,
                    KeptKind::Differs,
                    "a local file that differs from the listing".into(),
                );
            }
            return;
        };
        let current = identity::decode(&ph.identity);
        let same = matches!(&current, Some(Identity::File(f)) if f == want);
        if !ph.in_sync
            && let Some(aside) = &self.shared.set_aside
        {
            // A read-only view's file, changed here anyway (its read-only attribute is only an
            // attribute): Drive's version comes back, checked before the "unchanged" shortcut
            // below since an edit may keep the size and time (security review S3-2).
            return self.restore(n, d, &blob, aside, report);
        }
        if same && d.mtime_ms == n.mtime_ms {
            if self.shared.read_only && ph.in_sync && !d.readonly {
                // The attribute was cleared but nothing written yet: set again, so programs go
                // back to opening the file for reading only.
                let r = fs::metadata(&d.path).and_then(|m| {
                    let mut p = m.permissions();
                    p.set_readonly(true);
                    fs::set_permissions(&d.path, p)
                });
                if let Err(e) = r {
                    kept(
                        report,
                        KeptKind::Failed,
                        format!("could not make it read-only again: {e}"),
                    );
                }
            }
            return;
        }
        if !ph.in_sync {
            return kept(
                report,
                KeptKind::ChangedHere,
                "changed here and not uploaded yet".into(),
            );
        }
        let pinned = ph.pin == CF_PIN_STATE_PINNED;
        let r = cf::open(&d.path, true).and_then(|f| {
            if !same && pinned {
                // A pinned file cannot be dehydrated; pinned again below, the watcher then
                // fetches the new content.
                cf::set_pin(&f, CF_PIN_STATE_UNSPECIFIED, false)?;
            }
            cf::update(&f, want.size, n.mtime_ms, &blob, !same, self.shared.read_only)?;
            if !same && pinned {
                cf::set_pin(&f, CF_PIN_STATE_PINNED, false)?;
            }
            Ok(())
        });
        match r {
            Ok(()) => {
                if !same && let Some(Identity::File(old)) = current {
                    self.shared.hydrator.cache().forget(&old);
                }
                report.updated += 1;
            }
            Err(e) => kept(report, KeptKind::Failed, format!("could not update: {e}")),
        }
    }
}

impl OnDemandRoot {
    /// Puts Drive's version back in a read-only root's placeholder changed here: its content is
    /// copied to `aside` first (nothing the person wrote is lost), then the placeholder is
    /// marked in sync and dehydrated, read again on demand. A copy that fails leaves the file
    /// alone (reported, tried again on the next sync).
    ///
    /// The copy and the dehydrate go through one handle that lets nobody else write: bytes
    /// written between a copy and the dehydrate would otherwise be lost. A file another program
    /// still has open for writing is left alone this time (reported) and restored at a later
    /// sync, once it is closed.
    fn restore(&self, n: &Node, d: &OnDisk, blob: &[u8], aside: &std::path::Path, report: &mut SyncReport) {
        let Some(want) = &n.file else { return };
        let f = match cf::open_alone(&d.path) {
            Ok(f) => f,
            Err(e) => {
                return report.kept.push(Kept::new(
                    &n.path,
                    KeptKind::Failed,
                    format!(
                        "changed here and still open in another program; Drive's version comes back once it is closed ({e})"
                    ),
                ));
            }
        };
        // Its state now, with nobody else writing: the walk's may be from before a last write.
        let ph = match cf::info(&f) {
            Ok(Some(now)) => now,
            Ok(None) => return,
            Err(e) => {
                return report.kept.push(Kept::new(
                    &n.path,
                    KeptKind::Failed,
                    format!("changed here; could not read its state: {e}"),
                ));
            }
        };
        let ph = &ph;
        let mut copy = None;
        if ph.on_disk > 0 {
            // A dehydrated placeholder holds nothing written here: only its state changed
            // (attributes), and nothing needs keeping.
            match copy_aside(&f, aside, &n.path) {
                Ok(to) => copy = Some(to),
                Err(e) => {
                    return report.kept.push(Kept::new(
                        &n.path,
                        KeptKind::Failed,
                        format!("changed here; could not set the changed copy aside: {e}"),
                    ));
                }
            }
        }
        let pinned = ph.pin == CF_PIN_STATE_PINNED;
        let r = (|| {
            if pinned {
                cf::set_pin(&f, CF_PIN_STATE_UNSPECIFIED, false)?;
            }
            cf::update(&f, want.size, n.mtime_ms, blob, true, true)?;
            if pinned {
                // Kept on this computer: the watcher fetches Drive's version again.
                cf::set_pin(&f, CF_PIN_STATE_PINNED, false)?;
            }
            Ok::<(), std::io::Error>(())
        })();
        drop(f);
        match r {
            Ok(()) => {
                if pinned && let Some(w) = &self.watcher {
                    w.poke(d.path.clone());
                }
                if let Some(to) = copy {
                    report.restored.push((n.path.clone(), to));
                }
            }
            Err(e) => report.kept.push(Kept::new(
                &n.path,
                KeptKind::Failed,
                format!("changed here; could not put Drive's version back: {e}"),
            )),
        }
    }
}

impl OnDemandRoot {
    /// A read-only root's file changed here, freed when no listing is at hand (a vault's view
    /// locked, or cleared at start): its content goes to `aside` first, as [`Self::restore`] does,
    /// then the placeholder is marked in sync with the file it already names and dehydrated, read
    /// again on demand once there is something to read it from. A file still open for writing,
    /// or whose copy fails, is left as it is (the error says which). Returns where the copy went.
    pub(crate) fn set_aside_and_free(&self, p: &Path, aside: &Path) -> Result<Option<PathBuf>> {
        let rel = p
            .strip_prefix(self.root())
            .map_err(|_| Error::Outside(p.display().to_string()))?
            .to_string_lossy()
            .replace('\\', "/");
        let f = cf::open_alone(p).map_err(|e| os("opening alone", p, e))?;
        let Some(ph) = cf::info(&f).map_err(|e| os("reading the placeholder", p, e))? else {
            return Ok(None);
        };
        let Some(Identity::File(file)) = identity::decode(&ph.identity) else {
            return Err(Error::NotOurs(rel));
        };
        let copy = if ph.on_disk > 0 {
            Some(copy_aside(&f, aside, &rel).map_err(|e| os("setting the changed copy aside", p, e))?)
        } else {
            None
        };
        let mtime = fs::metadata(p).map(|m| mtime_ms(&m)).unwrap_or(0);
        let pinned = ph.pin == CF_PIN_STATE_PINNED;
        let r = (|| {
            if pinned {
                cf::set_pin(&f, CF_PIN_STATE_UNSPECIFIED, false)?;
            }
            cf::update(&f, file.size, mtime, &ph.identity, true, true)?;
            if pinned {
                // Still the person's choice: Drive's version comes back once it can be read.
                cf::set_pin(&f, CF_PIN_STATE_PINNED, false)?;
            }
            Ok::<(), std::io::Error>(())
        })();
        r.map_err(|e| os("freeing up space", p, e))?;
        Ok(copy)
    }
}

/// Copies what is on the disk of `f` (a placeholder opened alone) to `aside/<now>/<rel>`.
fn copy_aside(f: &fs::File, aside: &Path, rel: &str) -> std::io::Result<PathBuf> {
    let stamp = std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |t| t.as_millis());
    let to = aside.join(stamp.to_string()).join(rel.replace('/', "\\"));
    let saved = (|| {
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent)?;
        }
        // Read through a plain handle: a whole file copy API would also copy the placeholder's
        // attributes. Only content on the disk is read (Wasl's own reads of missing ranges are
        // refused), so a partly fetched file fails here.
        let mut out = fs::File::create(&to)?;
        std::io::copy(&mut &*f, &mut out)?;
        out.sync_all()
    })();
    match saved {
        Ok(()) => Ok(to),
        Err(e) => {
            let _ = fs::remove_file(&to);
            Err(e)
        }
    }
}

fn mtime_ms(m: &fs::Metadata) -> i64 {
    m.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as i64)
}
