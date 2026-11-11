//! The one long-lived connection to `wasld` (FR-E4): connects at start (starting the daemon when it
//! is not running), forwards its events to the window, keeps the tray's status fresh, and reconnects
//! when the daemon restarts.

use serde::Serialize;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};
use tokio::sync::{Notify, RwLock};
use wasl_ipc::{Client, DaemonStatus, Event, Paths};

/// How long to wait for a daemon we started (or one that restarted) to accept connections.
const CONNECT_WINDOW: Duration = Duration::from_secs(10);
/// While the daemon is not running, look for it this often (it may be started by the CLI).
const IDLE_POLL: Duration = Duration::from_secs(5);
/// Re-read the status this often while nothing happens, so a missed event cannot leave it stale.
const QUIET_REFRESH: Duration = Duration::from_secs(30);
/// Progress events arrive many times a second; the tray re-reads the status at most this often.
const PROGRESS_REFRESH: Duration = Duration::from_secs(1);

/// The connection as the window shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Conn {
    Connecting,
    /// We started `wasld` and wait for it.
    Starting,
    Connected { version: String },
    /// Not running and not started; `message` says why starting failed, if it did.
    NotRunning { message: Option<String> },
}

pub struct Daemon {
    client: RwLock<Option<Arc<Client>>>,
    conn: Mutex<Conn>,
    status: Mutex<Option<DaemonStatus>>,
    start_requested: AtomicBool,
    wake: Notify,
}

impl Daemon {
    pub fn new() -> Daemon {
        Daemon {
            client: RwLock::new(None),
            conn: Mutex::new(Conn::Connecting),
            status: Mutex::new(None),
            start_requested: AtomicBool::new(false),
            wake: Notify::new(),
        }
    }

    pub async fn client(&self) -> Option<Arc<Client>> {
        self.client.read().await.clone()
    }

    pub fn conn(&self) -> Conn {
        self.conn.lock().map(|c| c.clone()).unwrap_or(Conn::Connecting)
    }

    pub fn status(&self) -> Option<DaemonStatus> {
        self.status.lock().ok().and_then(|s| s.clone())
    }

    /// The "Start" button: start `wasld` now instead of waiting for the next poll.
    pub fn request_start(&self) {
        self.start_requested.store(true, Ordering::SeqCst);
        self.wake.notify_one();
    }

    fn set_conn(&self, app: &AppHandle, conn: Conn) {
        if let Ok(mut c) = self.conn.lock() {
            if *c == conn {
                return;
            }
            *c = conn.clone();
        }
        if !matches!(conn, Conn::Connected { .. })
            && let Ok(mut s) = self.status.lock()
        {
            *s = None;
        }
        let _ = app.emit("daemon://state", &conn);
        crate::tray::refresh(app);
    }

    /// Re-reads the status for the tray and the window.
    pub async fn refresh(&self, app: &AppHandle) {
        let Some(client) = self.client().await else {
            return;
        };
        if let Ok(status) = client.status().await {
            if let Ok(mut s) = self.status.lock() {
                *s = Some(status.clone());
            }
            let _ = app.emit("daemon://status", &status);
            crate::tray::refresh(app);
        }
    }
}

/// What to do after a connection attempt failed.
enum Next {
    /// Start `wasld`, then connect.
    Spawn,
    /// Connect only (the daemon was running a moment ago and may be restarting).
    Reconnect,
}

/// Runs for the life of the app.
pub async fn run(app: AppHandle, d: Arc<Daemon>) {
    let paths = match Paths::for_user() {
        Ok(p) => p,
        Err(e) => {
            d.set_conn(
                &app,
                Conn::NotRunning {
                    message: Some(e.to_string()),
                },
            );
            return;
        }
    };
    let mut next = Next::Spawn;
    let mut ready: Option<Client> = None;
    loop {
        if let Some(client) = ready.take() {
            next = after_serving(&app, &d, &paths, client, &mut ready).await;
            continue;
        }
        let attempt = match next {
            Next::Spawn => {
                d.set_conn(&app, Conn::Connecting);
                match Client::connect(&paths).await {
                    Ok(c) => Ok(c),
                    Err(_) => {
                        d.set_conn(&app, Conn::Starting);
                        match spawn_wasld() {
                            Ok(()) => connect_within(&paths, CONNECT_WINDOW)
                                .await
                                .ok_or(None),
                            Err(msg) => Err(Some(msg)),
                        }
                    }
                }
            }
            Next::Reconnect => {
                d.set_conn(&app, Conn::Connecting);
                connect_within(&paths, CONNECT_WINDOW).await.ok_or(None)
            }
        };
        let client = match attempt {
            Ok(c) => c,
            Err(message) => {
                d.set_conn(&app, Conn::NotRunning { message });
                match wait_while_down(&d, &paths).await {
                    Some(c) => c,
                    None => {
                        next = Next::Spawn;
                        continue;
                    }
                }
            }
        };
        next = after_serving(&app, &d, &paths, client, &mut ready).await;
    }
}

/// Serves a connection; decides what comes after it ends.
async fn after_serving(
    app: &AppHandle,
    d: &Daemon,
    paths: &Paths,
    client: Client,
    ready: &mut Option<Client>,
) -> Next {
    if !serve(app, d, client).await {
        return Next::Reconnect;
    }
    // The daemon was stopped on purpose: do not start it again behind the user's back.
    d.set_conn(app, Conn::NotRunning { message: None });
    match wait_while_down(d, paths).await {
        Some(c) => {
            *ready = Some(c);
            Next::Reconnect
        }
        None => Next::Spawn,
    }
}

/// While the daemon is down: poll quietly; returns a client when it came back by itself, or None
/// when the user asked to start it.
async fn wait_while_down(d: &Daemon, paths: &Paths) -> Option<Client> {
    loop {
        if d.start_requested.swap(false, Ordering::SeqCst) {
            return None;
        }
        tokio::select! {
            _ = d.wake.notified() => {}
            _ = tokio::time::sleep(IDLE_POLL) => {
                if let Ok(c) = Client::connect(paths).await {
                    return Some(c);
                }
            }
        }
    }
}

async fn connect_within(paths: &Paths, window: Duration) -> Option<Client> {
    let deadline = Instant::now() + window;
    loop {
        if let Ok(c) = Client::connect(paths).await {
            return Some(c);
        }
        if Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

/// Uses one connection until it ends. Returns true when the daemon said it was shutting down.
async fn serve(app: &AppHandle, d: &Daemon, client: Client) -> bool {
    let client = Arc::new(client);
    let Ok(mut events) = client.subscribe().await else {
        return false;
    };
    let version = client.daemon_version.clone();
    // General's "Show Wasl in the menu bar / notification area" (#71), kept by the daemon like every setting; unset
    // means shown. It wins over the local copy the icon started from.
    if let Ok(shown) = client.setting(crate::tray::VISIBLE_SETTING).await {
        crate::tray::set_visible(app, shown.as_deref() != Some("false"));
    }
    *d.client.write().await = Some(client);
    d.set_conn(app, Conn::Connected { version });
    d.refresh(app).await;
    crate::notify::check(app, d).await;
    let mut last_refresh = Instant::now();
    let mut shut_down = false;
    loop {
        tokio::select! {
            ev = events.recv() => {
                let Some(ev) = ev else { break };
                let _ = app.emit("daemon://event", &ev);
                match ev {
                    Event::ShuttingDown => {
                        shut_down = true;
                        break;
                    }
                    Event::Progress { .. } if last_refresh.elapsed() < PROGRESS_REFRESH => {}
                    _ => {
                        d.refresh(app).await;
                        crate::notify::check(app, d).await;
                        last_refresh = Instant::now();
                    }
                }
            }
            _ = tokio::time::sleep(QUIET_REFRESH) => {
                d.refresh(app).await;
                crate::notify::check(app, d).await;
                last_refresh = Instant::now();
            }
        }
    }
    *d.client.write().await = None;
    shut_down
}

fn exe_name(base: &str) -> String {
    format!("{base}{}", std::env::consts::EXE_SUFFIX)
}

/// A binary shipped next to this app's executable.
pub fn sibling(base: &str) -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let p = exe.parent()?.join(exe_name(base));
    p.is_file().then_some(p)
}

/// Starts `wasld` detached: next to the app first, then from PATH. It keeps running after the tray
/// quits.
fn spawn_wasld() -> Result<(), String> {
    use std::process::{Command, Stdio};
    let program = sibling("wasld").unwrap_or_else(|| PathBuf::from(exe_name("wasld")));
    let mut cmd = Command::new(&program);
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Its own process group, so a Ctrl+C meant for the tray's terminal does not reach it.
        cmd.process_group(0);
    }
    match cmd.spawn() {
        Ok(mut child) => {
            // Reap it when it exits so it does not linger as a zombie on Unix.
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err("wasld_not_found".into()),
        Err(e) => Err(e.to_string()),
    }
}
