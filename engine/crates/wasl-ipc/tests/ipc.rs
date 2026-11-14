//! The protocol on a real socket of this OS (a Unix socket or a Windows named pipe).

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use wasl_ipc::{
    Client, ClientError, ErrorKind, Event, Handler, Passphrase, Paths, RemoteError, Request, Server, Token,
    VaultInfo, ok,
};

struct Fake {
    calls: AtomicUsize,
}

#[async_trait::async_trait]
impl Handler for Fake {
    async fn handle(&self, request: Request) -> Result<serde_json::Value, RemoteError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        match request {
            Request::Pairs => ok(Vec::<wasl_proto::Pair>::new()),
            Request::Setting { key } if key == "slow" => {
                tokio::time::sleep(Duration::from_millis(300)).await;
                ok(Some("slow"))
            }
            Request::Setting { key } => ok(Some(format!("value of {key}"))),
            Request::AddView { view } => ok(view_from(&view)),
            Request::Views => ok(wasl_ipc::ViewsInfo {
                supported: true,
                views: vec![wasl_ipc::ViewState {
                    view: view_from(&new_view()),
                    mounted: true,
                    files: 2,
                    bytes: 30,
                    cache_used: 10,
                    pinned_bytes: 10,
                    local_changes: vec![wasl_ipc::LocalChange {
                        path: "notes.txt".into(),
                        reason: "only here".into(),
                        kind: Some(wasl_ipc::LocalChangeKind::OnlyHere),
                    }],
                    last_error_kind: Some(wasl_ipc::ErrorKind::Network),
                }],
            }),
            Request::PinPath { path } | Request::UnpinPath { path } if path.starts_with("/views/") => ok(()),
            Request::PinPath { path } => Err(RemoteError {
                kind: ErrorKind::NotFound,
                message: format!("{path} is not in a view"),
            }),
            Request::RemoveView { id, keep_files: true } if id == "v1" => ok(()),
            Request::Vaults => ok(vec![vault_info("vault-1", false)]),
            Request::CreateVault { vault } => {
                assert_eq!("correct horse", vault.passphrase.expose());
                ok(VaultInfo {
                    name: vault.name.clone(),
                    remote_path: vault.remote_path.clone(),
                    ..vault_info("vault-2", true)
                })
            }
            Request::UnlockVault { id, passphrase } if passphrase.expose() == "correct horse" => {
                ok(vault_info(&id, true))
            }
            Request::UnlockVault { .. } => Err(RemoteError {
                kind: ErrorKind::Invalid,
                message: "Wrong passphrase for this vault.".into(),
            }),
            Request::UnlockVaultFolder { folder, passphrase } => {
                assert_eq!("correct horse", passphrase.expose());
                ok(VaultInfo {
                    drive_folder_id: Some(folder.folder_id),
                    ..vault_info("vault-3", true)
                })
            }
            Request::LockVault { id } if id == "vault-1" => ok(()),
            Request::Pair { id } => Err(RemoteError {
                kind: ErrorKind::NotFound,
                message: format!("No pair {id}"),
            }),
            _ => Err(RemoteError {
                kind: ErrorKind::Unsupported,
                message: "not in this test".into(),
            }),
        }
    }
}

fn new_view() -> wasl_ipc::NewView {
    wasl_ipc::NewView {
        name: "Photos".into(),
        account_id: "me@example.com".into(),
        drive_folder_id: "F".into(),
        drive_id: None,
        remote_path: "My Drive/Photos".into(),
        local_path: "/views/Photos".into(),
        cache_limit: Some(1 << 30),
    }
}

fn view_from(n: &wasl_ipc::NewView) -> wasl_ipc::View {
    wasl_ipc::View {
        id: "v1".into(),
        name: n.name.clone(),
        account_id: n.account_id.clone(),
        drive_folder_id: n.drive_folder_id.clone(),
        drive_id: n.drive_id.clone(),
        remote_path: n.remote_path.clone(),
        local_path: n.local_path.clone(),
        vault_id: None,
        cache_limit: n.cache_limit.unwrap_or(0),
        enabled: true,
        created_at: 1,
        last_refresh_at: None,
        last_error: None,
    }
}

fn vault_info(id: &str, unlocked: bool) -> VaultInfo {
    VaultInfo {
        id: id.into(),
        name: "Private".into(),
        version: 2,
        format: "2.1".into(),
        unlocked,
        account_id: Some("me@example.com".into()),
        drive_folder_id: Some("F".into()),
        drive_id: None,
        remote_path: "My Drive/Private".into(),
        created_at: 1,
    }
}

fn start(dir: &std::path::Path) -> (Paths, tokio::sync::broadcast::Sender<Event>, Arc<Fake>) {
    let paths = Paths::at(dir);
    let fake = Arc::new(Fake {
        calls: AtomicUsize::new(0),
    });
    let server = Server::bind(&paths, fake.clone(), "test").unwrap();
    let events = server.events();
    tokio::spawn(server.serve());
    (paths, events, fake)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_client_of_this_user_calls_the_daemon() {
    let dir = tempfile::tempdir().unwrap();
    let (paths, _, _) = start(dir.path());
    let c = Client::connect(&paths).await.unwrap();
    assert_eq!("test", c.daemon_version);
    assert!(c.pairs().await.unwrap().is_empty());
    assert_eq!(Some("value of x".into()), c.setting("x").await.unwrap());

    let err = c.pair("nope").await.unwrap_err();
    assert_eq!(Some(ErrorKind::NotFound), err.kind());
    assert_eq!("No pair nope", err.to_string());
}

#[tokio::test(flavor = "multi_thread")]
async fn calls_on_one_connection_run_concurrently() {
    let dir = tempfile::tempdir().unwrap();
    let (paths, _, _) = start(dir.path());
    let c = Arc::new(Client::connect(&paths).await.unwrap());
    let slow = {
        let c = c.clone();
        tokio::spawn(async move { c.setting("slow").await })
    };
    tokio::time::sleep(Duration::from_millis(30)).await;
    // The fast answer arrives while the slow one is still running.
    let fast = tokio::time::timeout(Duration::from_millis(200), c.setting("fast")).await;
    assert_eq!(Some("value of fast".into()), fast.unwrap().unwrap());
    assert_eq!(Some("slow".into()), slow.await.unwrap().unwrap());
}

#[tokio::test(flavor = "multi_thread")]
async fn subscribed_clients_receive_events() {
    let dir = tempfile::tempdir().unwrap();
    let (paths, events, _) = start(dir.path());
    let c = Client::connect(&paths).await.unwrap();
    let mut rx = c.subscribe().await.unwrap();
    let quiet = Client::connect(&paths).await.unwrap();
    events.send(Event::Changed).unwrap();
    let got = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .unwrap();
    assert!(matches!(got, Some(Event::Changed)));
    // A connection that did not subscribe still answers calls normally.
    assert!(quiet.pairs().await.unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_client_without_the_token_is_refused_before_any_request() {
    let dir = tempfile::tempdir().unwrap();
    let (paths, _, fake) = start(dir.path());
    // Another token: a process that cannot read this user's data folder.
    let other = tempfile::tempdir().unwrap();
    let wrong = Paths::at(other.path());
    Token::create(&wrong.token()).unwrap();
    std::fs::copy(wrong.token(), dir.path().join("stolen.token")).unwrap();
    let real = std::fs::read(paths.token()).unwrap();
    std::fs::copy(wrong.token(), paths.token()).unwrap();
    let r = Client::connect(&paths).await;
    assert!(matches!(r, Err(ClientError::Untrusted)), "{:?}", r.err());
    assert_eq!(0, fake.calls.load(Ordering::SeqCst));
    std::fs::write(paths.token(), real).unwrap();
    assert!(Client::connect(&paths).await.is_ok());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_second_daemon_for_the_same_user_cannot_start() {
    let dir = tempfile::tempdir().unwrap();
    let (paths, _, _) = start(dir.path());
    let fake = Arc::new(Fake {
        calls: AtomicUsize::new(0),
    });
    let second = Server::bind(&paths, fake, "second");
    assert!(second.is_err());
    // The first daemon's token was not replaced.
    assert_eq!("test", Client::connect(&paths).await.unwrap().daemon_version);
}

#[tokio::test(flavor = "multi_thread")]
async fn no_daemon_means_not_running() {
    let dir = tempfile::tempdir().unwrap();
    let r = Client::connect(&Paths::at(dir.path())).await;
    assert!(matches!(r, Err(ClientError::NotRunning)));
}

#[tokio::test(flavor = "multi_thread")]
async fn views_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let (paths, _, _) = start(dir.path());
    let c = Client::connect(&paths).await.unwrap();
    let v = c.add_view(new_view()).await.unwrap();
    assert_eq!(view_from(&new_view()), v);
    let info = c.views().await.unwrap();
    assert!(info.supported);
    assert_eq!(1, info.views.len());
    assert_eq!(v, info.views[0].view);
    assert_eq!("notes.txt", info.views[0].local_changes[0].path);
    assert_eq!(Some(wasl_ipc::ErrorKind::Network), info.views[0].last_error_kind);
    c.pin_path("/views/Photos/a.jpg").await.unwrap();
    c.unpin_path("/views/Photos/a.jpg").await.unwrap();
    let err = c.pin_path("/elsewhere/a.jpg").await.unwrap_err();
    assert_eq!(Some(ErrorKind::NotFound), err.kind());
    c.remove_view("v1", true).await.unwrap();

    // The wire form clients other than Rust (the tray's TypeScript) rely on.
    let wire = serde_json::to_value(Request::RemoveView {
        id: "v1".into(),
        keep_files: false,
    })
    .unwrap();
    assert_eq!(
        serde_json::json!({"op": "remove_view", "id": "v1", "keep_files": false}),
        wire
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn vaults_round_trip_and_the_passphrase_is_never_printed() {
    let dir = tempfile::tempdir().unwrap();
    let (paths, _, _) = start(dir.path());
    let c = Client::connect(&paths).await.unwrap();
    assert_eq!(vec![vault_info("vault-1", false)], c.vaults().await.unwrap());

    let made = c
        .create_vault(wasl_ipc::NewVault {
            account_id: "me@example.com".into(),
            drive_id: None,
            parent_id: "root".into(),
            folder_id: None,
            remote_path: "My Drive/Taxes".into(),
            name: "Taxes".into(),
            passphrase: Passphrase::new("correct horse"),
        })
        .await
        .unwrap();
    assert_eq!(
        ("Taxes", "My Drive/Taxes", true),
        (made.name.as_str(), made.remote_path.as_str(), made.unlocked)
    );

    let wrong = c
        .unlock_vault("vault-1", Passphrase::new("wrong"))
        .await
        .unwrap_err();
    assert_eq!(Some(ErrorKind::Invalid), wrong.kind());
    assert!(
        c.unlock_vault("vault-1", Passphrase::new("correct horse"))
            .await
            .unwrap()
            .unlocked
    );
    let folder = wasl_ipc::VaultFolder {
        account_id: "me@example.com".into(),
        folder_id: "G".into(),
        drive_id: None,
        remote_path: "My Drive/Old vault".into(),
    };
    let found = c
        .unlock_vault_folder(folder, Passphrase::new("correct horse"))
        .await
        .unwrap();
    assert_eq!(Some("G".to_string()), found.drive_folder_id);
    c.lock_vault("vault-1").await.unwrap();

    // The wire form the tray's TypeScript sends; the passphrase is a plain string on the wire
    // but never in a debug print.
    let request = Request::UnlockVault {
        id: "vault-1".into(),
        passphrase: Passphrase::new("correct horse"),
    };
    assert!(!format!("{request:?}").contains("correct horse"));
    assert_eq!(
        serde_json::json!({"op": "unlock_vault", "id": "vault-1", "passphrase": "correct horse"}),
        serde_json::to_value(&request).unwrap()
    );
    assert_eq!(
        serde_json::json!({"op": "lock_vault", "id": "v"}),
        serde_json::to_value(Request::LockVault { id: "v".into() }).unwrap()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_long_line_before_the_handshake_is_refused() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let dir = tempfile::tempdir().unwrap();
    let (paths, _, fake) = start(dir.path());
    let mut raw = wasl_ipc::transport::connect(&paths).await.unwrap();
    // 64 KiB without a newline: far more than any hello, far less than MAX_LINE.
    let junk = vec![b'a'; 64 << 10];
    let _ = raw.write_all(&junk).await;
    let mut buf = [0u8; 16];
    let n = tokio::time::timeout(Duration::from_secs(2), raw.read(&mut buf))
        .await
        .expect("the daemon closes the connection before the handshake timeout");
    assert!(matches!(n, Ok(0) | Err(_)), "{n:?}");
    assert_eq!(0, fake.calls.load(Ordering::SeqCst));
}

#[tokio::test(flavor = "multi_thread")]
async fn connections_that_never_speak_are_capped() {
    use tokio::io::AsyncReadExt;
    let dir = tempfile::tempdir().unwrap();
    let (paths, _, _) = start(dir.path());
    let mut silent = Vec::new();
    for _ in 0..wasl_ipc::server::MAX_UNPROVEN {
        silent.push(wasl_ipc::transport::connect(&paths).await.unwrap());
    }
    // Let the daemon accept them all.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let mut over = wasl_ipc::transport::connect(&paths).await.unwrap();
    let mut buf = [0u8; 16];
    let n = tokio::time::timeout(Duration::from_secs(2), over.read(&mut buf))
        .await
        .expect("a connection over the cap is closed at once");
    assert!(matches!(n, Ok(0) | Err(_)), "{n:?}");
    // Once they go, this user's clients connect again.
    drop(silent);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(Client::connect(&paths).await.is_ok());
}

#[cfg(windows)]
#[tokio::test(flavor = "multi_thread")]
async fn a_squatted_pipe_name_is_reported_as_another_program() {
    use tokio::net::windows::named_pipe::ServerOptions;
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::at(dir.path());
    paths.ensure().unwrap();
    // The squatter holds the name and hangs up on whoever connects, without the token.
    let squat = ServerOptions::new()
        .first_pipe_instance(true)
        .create(paths.pipe_name())
        .unwrap();
    let squatter = tokio::spawn(async move {
        let _ = squat.connect().await;
        drop(squat);
    });
    Token::create(&paths.token()).unwrap();
    let fake = Arc::new(Fake {
        calls: AtomicUsize::new(0),
    });
    let err = Server::bind(&paths, fake, "test").err().unwrap();
    assert!(err.to_string().contains("in use by another program"), "{err}");
    squatter.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_running_daemon_is_still_reported_as_running() {
    let dir = tempfile::tempdir().unwrap();
    let (paths, _, _) = start(dir.path());
    let fake = Arc::new(Fake {
        calls: AtomicUsize::new(0),
    });
    let err = Server::bind(&paths, fake, "second").err().unwrap();
    assert!(err.to_string().contains("already running"), "{err}");
}

/// The token file's access list as SDDL, from PowerShell.
#[cfg(windows)]
fn sddl(path: &std::path::Path) -> String {
    let out = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command"])
        .arg(format!("(Get-Acl -LiteralPath '{}').Sddl", path.display()))
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

#[cfg(windows)]
#[tokio::test(flavor = "multi_thread")]
async fn a_portable_data_folder_is_limited_to_this_user() {
    let outer = tempfile::tempdir().unwrap();
    // A folder anyone may read, as a shared or hand-made portable folder can be.
    let st = std::process::Command::new("icacls")
        .arg(outer.path())
        .args(["/grant", "*S-1-1-0:(OI)(CI)R", "*S-1-5-32-545:(OI)(CI)R", "/Q"])
        .status()
        .unwrap();
    assert!(st.success());
    let paths = Paths::at(outer.path().join("home"));
    let fake = Arc::new(Fake {
        calls: AtomicUsize::new(0),
    });
    let server = Server::bind(&paths, fake, "test").unwrap();
    drop(server);
    for path in [paths.data.clone(), paths.token()] {
        let sddl = sddl(&path);
        let dacl = &sddl[sddl.find("D:").expect("a DACL")..];
        // Protected: nothing inherited from the shared parent.
        assert!(dacl.starts_with("D:P"), "{path:?}: {sddl}");
        // Everyone, Users, Authenticated Users, by alias or by SID.
        for broad in [";WD)", ";BU)", ";AU)", "S-1-1-0)", "S-1-5-32-545)", "S-1-5-11)"] {
            assert!(!dacl.contains(broad), "{path:?} grants {broad}: {sddl}");
        }
        assert!(dacl.contains(";SY)"), "{path:?}: {sddl}");
    }
    // This user still reads its own token.
    Token::read(&paths.token()).unwrap();
}
