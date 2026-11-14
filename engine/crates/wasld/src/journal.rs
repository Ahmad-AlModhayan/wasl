//! Agent undo's watcher (docs/AGENT_UNDO.md, UND-3 and UND-4): while a session is open, watches
//! every watched folder it snapshotted and journals each change. An overflow, a watcher error, a
//! removed folder or a daemon restart leads to a rescan against what the session last knew.

use crate::daemon::Daemon;
use notify::event::{ModifyKind, RenameMode};
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use wasl_store::ChangesetStatus;

/// What a watcher saw in one session's watched folder.
enum Seen {
    Paths {
        cs: String,
        watch: String,
        rels: Vec<String>,
    },
    Rename {
        cs: String,
        watch: String,
        from: String,
        to: String,
    },
    Rescan {
        cs: String,
        why: String,
    },
}

/// `path` relative to `root`, `/`-separated; `None` outside it or for the root itself.
fn rel_of(root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    let parts: Vec<&str> = rel.iter().filter_map(|p| p.to_str()).collect();
    (!parts.is_empty() && parts.len() == rel.iter().count()).then(|| parts.join("/"))
}

fn watch(
    cs: &str,
    watch_id: &str,
    root: &str,
    tx: mpsc::UnboundedSender<Seen>,
) -> notify::Result<RecommendedWatcher> {
    let (cs, id, base) = (cs.to_owned(), watch_id.to_owned(), PathBuf::from(root));
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        let seen = match res {
            Err(e) => Seen::Rescan {
                cs: cs.clone(),
                why: format!("a watcher error ({e})"),
            },
            Ok(ev) if ev.need_rescan() => Seen::Rescan {
                cs: cs.clone(),
                why: "a watcher overflow".into(),
            },
            Ok(ev) if matches!(ev.kind, EventKind::Access(_)) => return,
            Ok(ev) => {
                let rels: Vec<String> = ev.paths.iter().filter_map(|p| rel_of(&base, p)).collect();
                match (ev.kind, rels.as_slice()) {
                    (EventKind::Modify(ModifyKind::Name(RenameMode::Both)), [from, to]) => Seen::Rename {
                        cs: cs.clone(),
                        watch: id.clone(),
                        from: from.clone(),
                        to: to.clone(),
                    },
                    (_, []) => return,
                    _ => Seen::Paths {
                        cs: cs.clone(),
                        watch: id.clone(),
                        rels,
                    },
                }
            }
        };
        let _ = tx.send(seen);
    })?;
    watcher.watch(Path::new(root), RecursiveMode::Recursive)?;
    Ok(watcher)
}

/// Where watched folder `watch` of session `cs` is.
fn root_of(daemon: &Daemon, cs: &str, watch: &str) -> Option<PathBuf> {
    let roots = daemon.store.lock().journal_roots(cs).unwrap_or_default();
    roots
        .into_iter()
        .find(|w| w.id == watch)
        .map(|w| PathBuf::from(w.root))
}

/// Runs until the daemon stops: every `tick`, starts watching sessions that began and stops for
/// sessions that ended; in between, journals what the watchers see.
pub async fn run(daemon: Arc<Daemon>, tick: Duration) {
    let (tx, rx) = mpsc::unbounded_channel::<Seen>();
    run_with(daemon, tick, tx, rx).await
}

async fn run_with(
    daemon: Arc<Daemon>,
    tick: Duration,
    tx: mpsc::UnboundedSender<Seen>,
    mut rx: mpsc::UnboundedReceiver<Seen>,
) {
    // Per session, its watchers by watched folder; a folder Wasl couldn't watch is tried again
    // each tick.
    let mut watching: HashMap<String, HashMap<String, RecommendedWatcher>> = HashMap::new();
    let mut failing: HashSet<(String, String)> = HashSet::new();
    let mut rescan: BTreeSet<(String, String)> = BTreeSet::new();
    let mut started = false;
    let mut ticker = tokio::time::interval(tick);
    loop {
        tokio::select! {
            _ = ticker.tick() => {
                let open: Vec<String> = daemon
                    .engine
                    .agent_changesets(None, false)
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|c| c.status == ChangesetStatus::Open)
                    .map(|c| c.id)
                    .collect();
                watching.retain(|cs, _| open.contains(cs));
                failing.retain(|(cs, _)| open.contains(cs));
                rescan.retain(|(cs, _)| open.contains(cs));
                for cs in open {
                    let new = !watching.contains_key(&cs);
                    let watchers = watching.entry(cs.clone()).or_default();
                    let roots = daemon.store.lock().journal_roots(&cs).unwrap_or_default();
                    let mut began = false;
                    for w in &roots {
                        if watchers.contains_key(&w.id) {
                            continue;
                        }
                        match watch(&cs, &w.id, &w.root, tx.clone()) {
                            Ok(watcher) => {
                                watchers.insert(w.id.clone(), watcher);
                                failing.remove(&(cs.clone(), w.id.clone()));
                                began = true;
                            }
                            Err(e) => {
                                if failing.insert((cs.clone(), w.id.clone())) {
                                    let why = format!("can't watch {}: {e}", w.root);
                                    if let Err(e) = daemon.engine.agent_watch_gap(&cs, &w.id, &why) {
                                        eprintln!("agent undo: session {cs}: {why} ({e})");
                                    }
                                }
                            }
                        }
                    }
                    if began || (new && !roots.is_empty()) {
                        // What changed before the watcher started: since the daemon's last run, or
                        // between the session's start (or a failed watch) and now.
                        let why = if started { "the watcher started" } else { "a daemon restart" };
                        rescan.insert((cs.clone(), why.to_owned()));
                    }
                }
                started = true;
                for (cs, why) in std::mem::take(&mut rescan) {
                    if let Err(e) = daemon.engine.agent_rescan(&cs, &why).await {
                        // The folders it couldn't rescan are listed as missed; tried again next tick.
                        eprintln!("agent undo: rescan of session {cs} failed: {e}");
                        rescan.insert((cs, why));
                    }
                }
            }
            Some(first) = rx.recv() => {
                // What else came in meanwhile: several events on one path give one capture of its
                // latest bytes, noted as such (UND-4).
                let mut batch = vec![first];
                while let Ok(more) = rx.try_recv() {
                    batch.push(more);
                }
                let mut times: HashMap<(String, String, String), usize> = HashMap::new();
                for seen in &batch {
                    if let Seen::Paths { cs, watch, rels } = seen {
                        for rel in rels {
                            *times.entry((cs.clone(), watch.clone(), rel.clone())).or_default() += 1;
                        }
                    }
                }
                for seen in batch {
                    journal(&daemon, seen, &mut times, &mut rescan).await;
                }
            }
        }
    }
}

/// Journals one thing a watcher saw. `times` counts the events per path still to be journaled
/// in this batch: a path is captured once, at its first event.
async fn journal(
    daemon: &Daemon,
    seen: Seen,
    times: &mut HashMap<(String, String, String), usize>,
    rescan: &mut BTreeSet<(String, String)>,
) {
    match seen {
        Seen::Rescan { cs, why } => {
            rescan.insert((cs, why));
        }
        Seen::Rename { cs, watch, from, to } => {
            // A folder renamed: what was in it is found by a rescan.
            let dir = root_of(daemon, &cs, &watch).is_some_and(|r| r.join(&to).is_dir());
            match daemon.engine.agent_capture_rename(&cs, &watch, &from, &to).await {
                Ok(v) if v.is_empty() || dir => {
                    rescan.insert((cs, "a folder changed".into()));
                }
                Ok(_) => {}
                Err(_) => {
                    rescan.insert((cs, "a change Wasl couldn't journal".into()));
                }
            }
        }
        Seen::Paths { cs, watch, rels } => {
            for rel in rels {
                let n = match times.remove(&(cs.clone(), watch.clone(), rel.clone())) {
                    Some(n) => n,
                    None => continue, // captured at an earlier event of this batch
                };
                let captured = match n {
                    1 => daemon.engine.agent_capture(&cs, &watch, &rel, None).await,
                    n => {
                        let note = format!("{n} changes came in together; versions in between were not seen");
                        daemon.engine.agent_capture_noted(&cs, &watch, &rel, &note).await
                    }
                };
                match captured {
                    // A folder renamed or removed: what was in it is found by a rescan.
                    Ok(None) => {
                        if root_of(daemon, &cs, &watch).is_some_and(|r| !r.join(&rel).is_file()) {
                            rescan.insert((cs.clone(), "a folder changed".into()));
                        }
                    }
                    Ok(Some(_)) => {}
                    Err(_) => {
                        rescan.insert((cs.clone(), "a change Wasl couldn't journal".into()));
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::DaemonConfig;
    use crate::oauth::OAuthConfig;
    use crate::tokens::MemorySecrets;
    use std::time::Instant;

    fn daemon(data: &Path) -> Arc<Daemon> {
        Daemon::open(DaemonConfig {
            data_dir: data.to_owned(),
            oauth: OAuthConfig {
                client_id: "test".into(),
                client_secret: "test".into(),
                auth_url: "http://127.0.0.1:9/auth".into(),
                token_url: "http://127.0.0.1:9/token".into(),
                userinfo_url: "http://127.0.0.1:9/userinfo".into(),
                scopes: vec![],
            },
            secrets: Arc::new(MemorySecrets::default()),
            vault_keys: Arc::new(MemorySecrets::default()),
            drive_endpoints: None,
            open_browser: false,
            device_name: "Test PC".into(),
            sign_in_timeout: Duration::from_secs(10),
            devices: None,
            view_backend: None,
            view_refresh: Duration::from_secs(300),
        })
        .unwrap()
    }

    /// Paths of `root` that are files, `/`-separated, with their bytes.
    fn files(root: &Path, base: &Path, out: &mut Vec<(String, Vec<u8>)>) {
        for e in std::fs::read_dir(root).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                files(&p, base, out);
            } else {
                out.push((rel_of(base, &p).unwrap(), std::fs::read(&p).unwrap()));
            }
        }
    }

    /// What isn't yet restorable or listed: each file as it is now must be the session's last
    /// kept version of it, and each removed one journaled as deleted, unless the report lists it.
    fn unaccounted(daemon: &Daemon, cs: &str, watch: &str, site: &Path, removed: &[&str]) -> Vec<String> {
        let report = daemon.engine.agent_journal(cs).unwrap();
        let listed = |rel: &str| {
            report.not_captured.iter().any(|e| e.rel == rel)
                || report
                    .gaps
                    .iter()
                    .any(|g| g.rel.as_deref().is_none_or(|r| r == rel))
        };
        let last = |rel: &str| {
            daemon
                .engine
                .agent_versions(cs, watch, rel)
                .unwrap_or_default()
                .pop()
        };
        let mut now = Vec::new();
        files(site, site, &mut now);
        let mut out = Vec::new();
        for (rel, bytes) in now {
            let kept = last(&rel)
                .and_then(|v| v.path)
                .and_then(|p| std::fs::read(p).ok())
                == Some(bytes);
            if !kept && !listed(&rel) {
                out.push(rel);
            }
        }
        for rel in removed {
            if !last(rel).is_some_and(|v| v.blake3.is_none()) && !listed(rel) {
                out.push((*rel).to_owned());
            }
        }
        out
    }

    /// Waits, generously and bounded, for every file to be restorable or listed.
    async fn settled(daemon: &Daemon, cs: &str, watch: &str, site: &Path, removed: &[&str]) {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let left = unaccounted(daemon, cs, watch, site, removed);
            if left.is_empty() {
                return;
            }
            assert!(Instant::now() < deadline, "not restorable or listed: {left:?}");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_real_watcher_leaves_every_file_restorable_or_listed() {
        let dir = tempfile::tempdir().unwrap();
        let base = std::fs::canonicalize(dir.path()).unwrap();
        let (sync, site) = (base.join("Sync"), base.join("site"));
        std::fs::create_dir_all(&sync).unwrap();
        std::fs::create_dir_all(site.join("dir")).unwrap();
        std::fs::write(site.join("notes.md"), "version 0").unwrap();
        std::fs::write(site.join("dir/a.txt"), "a").unwrap();
        std::fs::write(site.join("dir/b.txt"), "b").unwrap();
        let d = daemon(&base.join("data"));
        d.store
            .lock()
            .create_pair(&wasl_store::Pair::new("p1", "Sync", sync.to_str().unwrap()))
            .unwrap();
        let watch = d.engine.agent_watch(&site).unwrap().id;
        let cs = d
            .engine
            .agent_begin("p1", "", "claude-code", None, None)
            .await
            .unwrap()
            .id;
        let (tx, rx) = mpsc::unbounded_channel::<Seen>();
        let running = tokio::spawn(run_with(d.clone(), Duration::from_millis(50), tx.clone(), rx));
        // The watcher starts at the first tick.
        tokio::time::sleep(Duration::from_millis(500)).await;

        // A burst of changes to one file.
        for i in 1..=20 {
            std::fs::write(site.join("notes.md"), format!("version {i}")).unwrap();
        }
        settled(&d, &cs, &watch, &site, &[]).await;

        // A folder renamed: its files are found under the new name, gone from the old.
        std::fs::rename(site.join("dir"), site.join("moved")).unwrap();
        settled(&d, &cs, &watch, &site, &["dir/a.txt", "dir/b.txt"]).await;
        let a = d.engine.agent_versions(&cs, &watch, "dir/a.txt").unwrap();
        let first = a[0].path.as_ref().and_then(|p| std::fs::read(p).ok());
        assert_eq!(Some(b"a".to_vec()), first, "the old path still restores");

        // A forced rescan, as after an overflow: a change made meanwhile is found.
        std::fs::write(site.join("moved/a.txt"), "changed").unwrap();
        tx.send(Seen::Rescan {
            cs: cs.clone(),
            why: "a watcher overflow".into(),
        })
        .unwrap();
        settled(&d, &cs, &watch, &site, &["dir/a.txt", "dir/b.txt"]).await;
        running.abort();
    }

    #[test]
    fn paths_are_made_relative_to_the_watched_folder() {
        let root = Path::new("/w/site");
        assert_eq!(
            Some("a/b.txt".to_owned()),
            rel_of(root, Path::new("/w/site/a/b.txt"))
        );
        assert_eq!(None, rel_of(root, Path::new("/w/site")));
        assert_eq!(None, rel_of(root, Path::new("/w/other/a.txt")));
    }
}
