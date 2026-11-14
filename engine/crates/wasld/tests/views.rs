//! Read-only views (docs/ON_DEMAND.md §2.3) through the daemon's view manager, with an in-memory
//! Drive and, on Windows, real Cloud Files placeholders.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use wasl_drive::DriveError;
use wasl_engine::{MemoryRemote, Op};
use wasl_ipc::{ErrorKind, NewView, RemoteError, View};
use wasl_store::{Pair, SharedStore, Store};
use wasld::views::{Backend, Close, MemoryBackend, ViewRemote, ViewRemotes, Views};

struct Drive(Arc<MemoryRemote>);

impl ViewRemotes for Drive {
    fn open(&self, view: &View) -> Result<ViewRemote, RemoteError> {
        if view.vault_id.is_some() {
            return Err(RemoteError {
                kind: ErrorKind::Locked,
                message: wasld::views::VAULT_LOCKED.into(),
            });
        }
        Ok(ViewRemote::plain(self.0.clone()))
    }
}

struct Setup {
    views: Views,
    store: SharedStore,
    drive: Arc<MemoryRemote>,
    backend: Arc<MemoryBackend>,
    folder: String,
    dir: tempfile::TempDir,
}

impl Setup {
    fn local(&self, rel: &str) -> PathBuf {
        self.dir.path().join(rel)
    }

    fn new_view(&self, name: &str, local: &Path) -> NewView {
        NewView {
            name: name.into(),
            account_id: "me@example.com".into(),
            drive_folder_id: self.folder.clone(),
            drive_id: None,
            remote_path: "My Drive/Photos".into(),
            local_path: local.display().to_string(),
            cache_limit: Some(64 << 20),
        }
    }

    fn state(&self, id: &str) -> wasl_ipc::ViewState {
        self.views
            .info()
            .unwrap()
            .views
            .into_iter()
            .find(|v| v.view.id == id)
            .unwrap()
    }

    fn shown(&self, id: &str) -> Vec<String> {
        let mut v = self
            .backend
            .shown
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .unwrap_or_default();
        v.sort();
        v
    }
}

fn setup_with(every: Duration, backend: Option<Arc<dyn Backend>>) -> (Setup, Arc<dyn Backend>) {
    let dir = tempfile::tempdir().unwrap();
    let store = SharedStore::new(Store::open(dir.path().join("wasl.db")).unwrap());
    let drive = Arc::new(MemoryRemote::new());
    let folder = drive.mkdirs("Photos");
    drive.put_file("Photos/a.jpg", b"aaaaaaaaaa");
    drive.put_file("Photos/trip/b.jpg", b"bbbbb");
    drive.put_google_doc("Photos/notes", "application/vnd.google-apps.document");
    drive.put_file("Elsewhere/c.txt", b"not in the view");
    let memory = Arc::new(MemoryBackend::default());
    let chosen: Arc<dyn Backend> = backend.unwrap_or_else(|| memory.clone());
    let views = Views::new(
        store.clone(),
        &dir.path().join("data"),
        Arc::new(Drive(drive.clone())),
        chosen.clone(),
        every,
        Arc::new(|| {}),
    );
    (
        Setup {
            views,
            store,
            drive,
            backend: memory,
            folder,
            dir,
        },
        chosen,
    )
}

fn setup() -> Setup {
    setup_with(Duration::from_secs(300), None).0
}

#[tokio::test]
async fn a_view_shows_its_folder_and_refreshes() {
    let s = setup();
    let local = s.local("Photos view");
    let view = s.views.add(s.new_view("Photos", &local), &[]).await.unwrap();
    assert!(view.enabled);
    assert_eq!(64 << 20, view.cache_limit);
    assert!(view.last_refresh_at.is_some());
    assert_eq!(None, view.last_error);

    let st = s.state(&view.id);
    assert!(st.mounted);
    assert_eq!((2, 15), (st.files, st.bytes));
    // Google documents have no bytes to show; files outside the folder are not in the view.
    assert_eq!(vec!["a.jpg", "trip", "trip/b.jpg"], s.shown(&view.id));
    assert_eq!(
        0,
        s.drive.bytes_downloaded(),
        "nothing is fetched until it is read"
    );

    s.drive.put_file("Photos/new.jpg", b"new!");
    s.views.refresh_now(&view.id).await.unwrap();
    assert_eq!(vec!["a.jpg", "new.jpg", "trip", "trip/b.jpg"], s.shown(&view.id));
    assert_eq!(3, s.state(&view.id).files);
    assert_eq!(0, s.drive.bytes_uploaded(), "a view never uploads");
}

#[tokio::test]
async fn pinned_files_stay_and_unpinned_ones_may_go() {
    let s = setup();
    let local = s.local("View");
    let view = s.views.add(s.new_view("Photos", &local), &[]).await.unwrap();

    s.views
        .set_pinned(&local.join("a.jpg").display().to_string(), true)
        .await
        .unwrap();
    let st = s.state(&view.id);
    assert_eq!(10, st.pinned_bytes);
    assert_eq!(10, s.drive.bytes_downloaded());

    // A folder pins every file in it; the whole view pins everything.
    s.views
        .set_pinned(&local.join("trip").display().to_string(), true)
        .await
        .unwrap();
    assert_eq!(15, s.state(&view.id).pinned_bytes);
    s.views
        .set_pinned(&local.display().to_string(), false)
        .await
        .unwrap();
    assert_eq!(0, s.state(&view.id).pinned_bytes);
    assert_eq!(
        15,
        s.state(&view.id).cache_used,
        "unpinned content stays until space is needed"
    );

    let outside = s
        .views
        .set_pinned(&s.local("other/a.jpg").display().to_string(), true)
        .await
        .unwrap_err();
    assert_eq!(ErrorKind::NotFound, outside.kind);
    let missing = s
        .views
        .set_pinned(&local.join("nope.jpg").display().to_string(), true)
        .await
        .unwrap_err();
    assert_eq!(ErrorKind::NotFound, missing.kind);
}

#[tokio::test]
async fn a_folder_with_someone_elses_vault_json_stays_a_plain_view() {
    let s = setup();
    let folder = s.drive.mkdirs("Notes");
    s.drive
        .put_file("Notes/vault.json", b"{\"format\":\"another app\"}");
    s.drive.put_file("Notes/todo.txt", b"milk");
    let mut new = s.new_view("Notes", &s.local("Notes view"));
    new.drive_folder_id = folder;
    let view = s.views.add(new, &[]).await.unwrap();
    assert_eq!(None, view.vault_id);
    assert!(s.state(&view.id).mounted);
    assert_eq!(vec!["todo.txt", "vault.json"], s.shown(&view.id));
}

#[tokio::test]
async fn a_view_recorded_on_a_locked_vault_waits_for_its_keys() {
    let s = setup();
    let vault = s.drive.mkdirs("Vault");
    let view = View {
        id: "vault-view".into(),
        name: "Vault".into(),
        account_id: "me@example.com".into(),
        drive_folder_id: vault,
        drive_id: None,
        remote_path: "My Drive/Vault".into(),
        local_path: s.local("Vault view").display().to_string(),
        vault_id: Some("vault-1".into()),
        cache_limit: 1 << 30,
        enabled: true,
        created_at: 1,
        last_refresh_at: None,
        last_error: None,
    };
    s.store.lock().create_view(&view).unwrap();
    s.views.tick().await;
    let st = s.state("vault-view");
    assert!(!st.mounted);
    assert_eq!(Some(wasld::views::VAULT_LOCKED), st.view.last_error.as_deref());
    assert_eq!(Some(ErrorKind::Locked), st.last_error_kind);
    assert!(wasld::views::VAULT_LOCKED.contains("Unlock the vault"));
    assert!(s.backend.mounts.lock().unwrap().is_empty());
}

#[tokio::test]
async fn turning_off_changing_the_limit_and_removing() {
    let s = setup();
    let local = s.local("View");
    let view = s.views.add(s.new_view("Photos", &local), &[]).await.unwrap();
    let cache = s.dir.path().join("data").join("views").join(&view.id);
    assert!(cache.is_dir());

    s.views.set_enabled(&view.id, false).await.unwrap();
    assert!(!s.state(&view.id).mounted);
    assert_eq!(
        vec![(view.id.clone(), Close::Disconnect)],
        *s.backend.closes.lock().unwrap()
    );
    let off = s
        .views
        .set_pinned(&local.join("a.jpg").display().to_string(), true)
        .await
        .unwrap_err();
    assert_eq!(ErrorKind::Invalid, off.kind);

    s.views.set_enabled(&view.id, true).await.unwrap();
    assert!(s.state(&view.id).mounted);
    assert_eq!(2, s.backend.mounts.lock().unwrap().len());

    s.views.set_cache_limit(&view.id, 1 << 20).await.unwrap();
    assert_eq!(1 << 20, s.state(&view.id).view.cache_limit);

    s.views.remove(&view.id, false).await.unwrap();
    assert_eq!(
        Some(&(view.id.clone(), Close::Remove)),
        s.backend.closes.lock().unwrap().last()
    );
    assert!(s.views.info().unwrap().views.is_empty());
    assert!(!cache.exists(), "the view's cache goes with it");

    let again = s.views.add(s.new_view("Again", &local), &[]).await.unwrap();
    s.views.remove(&again.id, true).await.unwrap();
    assert_eq!(
        Some(&(again.id.clone(), Close::KeepFiles)),
        s.backend.closes.lock().unwrap().last()
    );
    assert_eq!(
        ErrorKind::NotFound,
        s.views.remove("nope", false).await.unwrap_err().kind
    );
}

#[tokio::test]
async fn local_changes_are_reported_never_acted_on() {
    let s = setup();
    let view = s
        .views
        .add(s.new_view("Photos", &s.local("View")), &[])
        .await
        .unwrap();
    s.backend.kept.lock().unwrap().push(wasl_vfs::Kept::new(
        "a.jpg",
        wasl_vfs::KeptKind::ChangedHere,
        "changed here and not uploaded yet",
    ));
    s.views.refresh_now(&view.id).await.unwrap();
    let st = s.state(&view.id);
    assert_eq!(1, st.local_changes.len());
    assert_eq!("a.jpg", st.local_changes[0].path);
    assert_eq!(
        Some(wasl_ipc::LocalChangeKind::ChangedHere),
        st.local_changes[0].kind
    );
    assert_eq!(0, s.drive.bytes_uploaded());
    assert_eq!(1, s.drive.files().keys().filter(|k| k.ends_with("a.jpg")).count());
}

/// Security review S3-2: a file changed in a read-only view gets Drive's version back at the next
/// refresh (on Windows; FUSE refuses writes), and the view keeps reporting it with where the
/// changed copy went, after later refreshes too.
#[tokio::test]
async fn files_put_back_from_drive_are_reported() {
    let s = setup();
    let view = s
        .views
        .add(s.new_view("Photos", &s.local("View")), &[])
        .await
        .unwrap();
    let copy = s
        .dir
        .path()
        .join("data/views")
        .join(&view.id)
        .join("changed-here/1/a.jpg");
    s.backend
        .restored
        .lock()
        .unwrap()
        .push(("a.jpg".into(), copy.clone()));
    s.views.refresh_now(&view.id).await.unwrap();
    s.views.refresh_now(&view.id).await.unwrap();
    let st = s.state(&view.id);
    assert_eq!(1, st.local_changes.len(), "{:?}", st.local_changes);
    let c = &st.local_changes[0];
    assert_eq!("a.jpg", c.path);
    assert_eq!(Some(wasl_ipc::LocalChangeKind::Restored), c.kind);
    assert_eq!(copy.display().to_string(), c.reason);
    assert_eq!(0, s.drive.bytes_uploaded(), "nothing written there is uploaded");
}

/// Security review S3-2: a view of a locked vault that was not showing when the vault was locked
/// (turned off, or the daemon stopped) may still hold content fetched in an earlier run. At the
/// next start its folder is cleared before anything else, once, and nothing is mounted.
#[tokio::test]
async fn views_of_a_locked_vault_are_cleared_at_start_before_anything_shows() {
    let s = setup();
    let vault = s.drive.mkdirs("Vault");
    let mut view = View {
        id: "vault-on".into(),
        name: "Vault".into(),
        account_id: "me@example.com".into(),
        drive_folder_id: vault,
        drive_id: None,
        remote_path: "My Drive/Vault".into(),
        local_path: s.local("Vault view").display().to_string(),
        vault_id: Some("vault-1".into()),
        cache_limit: 1 << 30,
        enabled: true,
        created_at: 1,
        last_refresh_at: None,
        last_error: None,
    };
    s.store.lock().create_view(&view).unwrap();
    view.id = "vault-off".into();
    view.enabled = false;
    view.local_path = s.local("Vault view 2").display().to_string();
    s.store.lock().create_view(&view).unwrap();
    // A plain view is not touched.
    s.views
        .add(s.new_view("Photos", &s.local("View")), &[])
        .await
        .unwrap();

    s.views.tick().await;
    let mut scrubs = s.backend.scrubs.lock().unwrap().clone();
    scrubs.sort();
    assert_eq!(
        vec![("vault-off".to_owned(), false), ("vault-on".to_owned(), false)],
        scrubs,
        "both cleared, neither showing"
    );
    assert!(
        !s.backend
            .mounts
            .lock()
            .unwrap()
            .iter()
            .any(|m| m.starts_with("vault"))
    );
    // Once per run: the next ticks leave them be.
    s.views.tick().await;
    s.views.refresh_now("vault-on").await.unwrap_err();
    assert_eq!(2, s.backend.scrubs.lock().unwrap().len());
    // Locking the vault again clears them again (it may have been unlocked meanwhile).
    s.views.vault_locked("vault-1").await;
    assert_eq!(4, s.backend.scrubs.lock().unwrap().len());
}

#[tokio::test]
async fn a_view_needs_a_folder_of_its_own() {
    let s = setup();
    let busy = s.local("Busy");
    std::fs::create_dir_all(&busy).unwrap();
    std::fs::write(busy.join("mine.txt"), b"mine").unwrap();
    let e = s.views.add(s.new_view("X", &busy), &[]).await.unwrap_err();
    assert_eq!(ErrorKind::Invalid, e.kind);
    assert!(e.message.contains("isn't empty"), "{e:?}");
    assert_eq!(b"mine".to_vec(), std::fs::read(busy.join("mine.txt")).unwrap());

    let pair_root = s.local("Synced");
    let pair = Pair::new("p1", "Synced", &pair_root.display().to_string());
    let e = s
        .views
        .add(s.new_view("X", &pair_root.join("inside")), &[pair])
        .await
        .unwrap_err();
    assert_eq!(ErrorKind::Invalid, e.kind);

    let first = s.local("Views/One");
    s.views.add(s.new_view("One", &first), &[]).await.unwrap();
    let e = s
        .views
        .add(s.new_view("Around", &s.local("Views")), &[])
        .await
        .unwrap_err();
    assert_eq!(ErrorKind::Invalid, e.kind);

    let mut relative = s.new_view("Rel", Path::new("relative/folder"));
    relative.local_path = "relative/folder".into();
    assert_eq!(
        ErrorKind::Invalid,
        s.views.add(relative, &[]).await.unwrap_err().kind
    );
    let mut nameless = s.new_view("  ", &s.local("Nameless"));
    nameless.name = "  ".into();
    assert_eq!(
        ErrorKind::Invalid,
        s.views.add(nameless, &[]).await.unwrap_err().kind
    );
}

#[tokio::test]
async fn a_failed_listing_is_recorded_and_tried_again() {
    let (s, _) = setup_with(Duration::ZERO, None);
    // The first mount, right after the view is recorded, lists the folder.
    s.drive.fail_next(Op::List, DriveError::Network("offline".into()));
    let view = s
        .views
        .add(s.new_view("Photos", &s.local("View")), &[])
        .await
        .unwrap();
    assert_eq!(Some("offline"), view.last_error.as_deref());
    let st = s.state(&view.id);
    assert!(!st.mounted);
    // Its kind travels with it, so the app can say it in the person's language.
    assert_eq!(Some(ErrorKind::Network), st.last_error_kind);

    s.views.tick().await;
    let st = s.state(&view.id);
    assert!(st.mounted);
    assert_eq!(None, st.view.last_error);
    assert_eq!(None, st.last_error_kind);
}

#[test]
fn paths_inside_a_view_are_relative_to_it() {
    use wasld::views::relative;
    let root = if cfg!(windows) {
        PathBuf::from(r"C:\Users\me\Views\Photos")
    } else {
        PathBuf::from("/home/me/Views/Photos")
    };
    assert_eq!(Some(String::new()), relative(&root, &root));
    assert_eq!(
        Some("trip/b.jpg".into()),
        relative(&root, &root.join("trip").join("b.jpg"))
    );
    assert_eq!(None, relative(&root, root.parent().unwrap()));
    assert_eq!(None, relative(&root, &root.parent().unwrap().join("Photos2")));
    if cfg!(windows) {
        assert_eq!(
            Some("Trip".into()),
            relative(&root, Path::new(r"c:\users\ME\views\photos\Trip"))
        );
    }
}

/// The real thing on Windows: read-only placeholders, a pin that keeps a file, and a removal
/// that leaves the pinned file as an ordinary one.
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread")]
async fn on_windows_a_view_is_read_only_placeholders() {
    let (s, backend) = setup_with(Duration::from_secs(300), Some(wasld::views::platform_backend()));
    assert!(backend.supported());
    let local = s.local("View");
    let view = s
        .views
        .add(s.new_view("Wasl test view", &local), &[])
        .await
        .unwrap();
    let result = async {
        assert_eq!(None, view.last_error, "{view:?}");
        let a = local.join("a.jpg");
        let b = local.join("trip").join("b.jpg");
        assert_eq!(10, std::fs::metadata(&a).unwrap().len());
        assert!(std::fs::metadata(&a).unwrap().permissions().readonly());
        assert!(std::fs::OpenOptions::new().write(true).open(&a).is_err());
        assert_eq!(5, std::fs::metadata(&b).unwrap().len());
        assert_eq!(0, s.drive.bytes_downloaded());

        s.views.set_pinned(&a.display().to_string(), true).await.unwrap();
        assert_eq!(b"aaaaaaaaaa".to_vec(), std::fs::read(&a).unwrap());
        assert_eq!(10, s.state(&view.id).pinned_bytes);

        s.views.remove(&view.id, false).await.unwrap();
        // The pinned file stays, as an ordinary writable file; the other placeholders go.
        assert_eq!(b"aaaaaaaaaa".to_vec(), std::fs::read(&a).unwrap());
        assert!(!std::fs::metadata(&a).unwrap().permissions().readonly());
        assert!(!b.exists());
        assert!(!backend.leftover(&view));
    }
    .await;
    // Never leave a sync root behind, whatever failed.
    if backend.leftover(&view) {
        let _ = s.views.remove(&view.id, false).await;
    }
    result
}

/// Security review S3-2, the real thing on Windows: a vault's view that was not showing when the
/// vault was locked (here: the daemon stopped with a file fetched, then the vault was locked)
/// lets go of that content at the next start, before anything is shown; a pinned file keeps it.
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread")]
async fn on_windows_a_locked_vaults_leftover_view_is_cleared_at_start() {
    use std::os::windows::fs::MetadataExt;
    /// Windows sets it on a placeholder whose content is not on the disk.
    const RECALL_ON_DATA_ACCESS: u32 = 0x0040_0000;
    let dehydrated = |p: &Path| std::fs::metadata(p).unwrap().file_attributes() & RECALL_ON_DATA_ACCESS != 0;

    let (s, backend) = setup_with(Duration::from_secs(300), Some(wasld::views::platform_backend()));
    let local = s.local("View");
    let view = s
        .views
        .add(s.new_view("Wasl test vault view", &local), &[])
        .await
        .unwrap();
    let a = local.join("a.jpg");
    let b = local.join("trip").join("b.jpg");
    let result = async {
        // Fetched in an earlier run: a pinned, then unpinned (its content stays); b kept.
        s.views.set_pinned(&a.display().to_string(), true).await.unwrap();
        s.views.set_pinned(&a.display().to_string(), false).await.unwrap();
        s.views.set_pinned(&b.display().to_string(), true).await.unwrap();
        assert!(!dehydrated(&a) && !dehydrated(&b));
        // E2E 2026-10: and then changed by hand (read-only by attribute only).
        let cleared = std::process::Command::new("attrib")
            .arg("-r")
            .arg(&a)
            .status()
            .unwrap();
        assert!(cleared.success());
        std::fs::write(&a, b"my edit").unwrap();
        s.views.close_all().await;
        // The folder turned out to be a vault, locked on this computer at the next start.
        s.store.lock().set_view_vault(&view.id, Some("vault-1")).unwrap();

        let next = Views::new(
            s.store.clone(),
            &s.dir.path().join("data"),
            Arc::new(Drive(s.drive.clone())),
            backend.clone(),
            Duration::from_secs(300),
            Arc::new(|| {}),
        );
        next.tick().await;
        assert!(!next.info().unwrap().views[0].mounted);
        assert!(dehydrated(&a), "the content fetched before is gone");
        assert!(!dehydrated(&b), "a pinned file keeps its content");
        assert_eq!(10, std::fs::metadata(&a).unwrap().len(), "still a placeholder");
        // What was written by hand is set aside, outside the view, and said.
        let aside = s
            .dir
            .path()
            .join("data")
            .join("views")
            .join(&view.id)
            .join("changed-here");
        let copies = files_under(&aside);
        assert_eq!(1, copies.len(), "{copies:?}");
        assert_eq!(b"my edit".to_vec(), std::fs::read(&copies[0]).unwrap());
        let said = next.info().unwrap().views[0]
            .view
            .last_error
            .clone()
            .unwrap_or_default();
        assert!(said.contains("set aside unencrypted"), "{said}");
        s.store.lock().set_view_vault(&view.id, None).unwrap();
        next.remove(&view.id, false).await.unwrap();
    }
    .await;
    if backend.leftover(&view) {
        let _ = s.store.lock().set_view_vault(&view.id, None);
        let _ = s.views.remove(&view.id, false).await;
    }
    result
}

/// Every file under `dir`, at any depth.
#[cfg(windows)]
fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(files_under(&p));
        } else {
            out.push(p);
        }
    }
    out
}
