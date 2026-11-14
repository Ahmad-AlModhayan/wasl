//! `wasl-miqyas`: two Wasl devices on one machine for Miqyas (docs/testing/MIQYAS.md). Built only
//! with the `miqyas` feature, next to a `wasld` built with it.
//!
//! - `up [--root DIR] [--wasld PATH]`: starts devices A and B (each a test `wasld` with its own
//!   folder and socket under DIR), pairs them, and syncs a devices-only pair at reach `lan` over
//!   loopback between `DIR/a/folder` and `DIR/b/folder`. Prints the layout as JSON.
//! - `wait [--root DIR] [--timeout SECS]`: waits until both folders hold the same files and
//!   neither device is running; prints the time it took, the files and the bytes as JSON.
//! - `down [--root DIR]`: stops both daemons and removes DIR.
//!
//! Pairing here has no person: the harness reads the four words on both devices and confirms
//! both when they match, which is the check a person makes. Release builds have no such binary.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, Instant};
use wasl_core::{Reach, Route, RunStatus};
use wasl_engine::Trigger;
use wasl_ipc::{Client, PairingStage, PairingStatus, Paths};
use wasl_store::Pair;

/// In the root, so `down` removes only folders `up` made.
const MARKER: &str = ".wasl-miqyas";
const STATE: &str = "miqyas.json";
const PAIR_ID: &str = "miqyas";

#[derive(Debug, Serialize, Deserialize)]
struct State {
    root: PathBuf,
    devices: Vec<Device>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Device {
    name: String,
    /// The daemon's folder: `WASL_HOME` for the `wasl` CLI against this device.
    home: PathBuf,
    /// The synced folder.
    folder: PathBuf,
    pid: u32,
    log: PathBuf,
    device_id: String,
    listen: String,
    pair_id: String,
}

type R<T> = Result<T, String>;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first().cloned() else {
        eprintln!("usage: wasl-miqyas up|wait|down [--root DIR] [--wasld PATH] [--timeout SECS]");
        return ExitCode::from(2);
    };
    let opt = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let rt = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => return fail(&e.to_string()),
    };
    let result = match cmd.as_str() {
        "up" => rt.block_on(up(
            opt("--root").map(PathBuf::from),
            opt("--wasld").map(PathBuf::from),
        )),
        "wait" => {
            let timeout = opt("--timeout").and_then(|s| s.parse().ok()).unwrap_or(120);
            root(opt("--root")).and_then(|r| rt.block_on(wait(&r, Duration::from_secs(timeout))))
        }
        "down" => root(opt("--root")).and_then(|r| rt.block_on(down(&r))),
        other => Err(format!("unknown command {other}")),
    };
    match result {
        Ok(json) => {
            println!("{json}");
            ExitCode::SUCCESS
        }
        Err(e) => fail(&e),
    }
}

fn fail(msg: &str) -> ExitCode {
    eprintln!("wasl-miqyas: {msg}");
    ExitCode::FAILURE
}

/// `--root`, else `WASL_MIQYAS_ROOT`.
fn root(flag: Option<String>) -> R<PathBuf> {
    flag.or_else(|| std::env::var("WASL_MIQYAS_ROOT").ok())
        .map(PathBuf::from)
        .ok_or_else(|| "--root DIR (or WASL_MIQYAS_ROOT) is needed".into())
}

async fn up(root: Option<PathBuf>, wasld: Option<PathBuf>) -> R<String> {
    let root = match root {
        Some(r) => r,
        None => std::env::temp_dir().join(format!("wasl-miqyas-{}", std::process::id())),
    };
    if root.exists()
        && std::fs::read_dir(&root)
            .map_err(|e| e.to_string())?
            .next()
            .is_some()
    {
        return Err(format!("{} is not empty", root.display()));
    }
    std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    let root = std::fs::canonicalize(&root).map_err(|e| e.to_string())?;
    std::fs::write(root.join(MARKER), b"made by wasl-miqyas up\n").map_err(|e| e.to_string())?;
    let wasld = match wasld {
        Some(w) => w,
        None => std::env::current_exe()
            .map_err(|e| e.to_string())?
            .with_file_name(format!("wasld{}", std::env::consts::EXE_SUFFIX)),
    };
    let mut devices = Vec::new();
    for name in ["a", "b"] {
        let d = start(&wasld, &root, name)?;
        devices.push(d);
        // Saved as soon as a daemon runs, so `down` can stop it if a later step fails.
        save(&State {
            root: root.clone(),
            devices: devices.clone(),
        })?;
    }
    let (a, b) = (ready(&devices[0]).await?, ready(&devices[1]).await?);
    pair(&a, &b).await?;
    for (d, c) in devices.iter_mut().zip([&a, &b]) {
        let info = c.devices().await.map_err(|e| e.to_string())?;
        d.listen = info.listen.clone().ok_or("a device has no listen address")?;
        d.device_id = info
            .devices
            .iter()
            .find(|x| x.this_device)
            .map(|x| x.id.clone())
            .ok_or("a device does not list itself")?;
    }
    a.connect_device(&devices[1].listen)
        .await
        .map_err(|e| e.to_string())?;
    b.connect_device(&devices[0].listen)
        .await
        .map_err(|e| e.to_string())?;
    devices[1].pair_id = share(&a, &b, &devices[0], &devices[1]).await?;
    for (d, c) in devices.iter().zip([&a, &b]) {
        // Starting from the preview is the person's go-ahead for a new pair's first sync.
        c.sync(&d.pair_id, Trigger::Preview)
            .await
            .map_err(|e| e.to_string())?;
    }
    for (d, c) in devices.iter().zip([&a, &b]) {
        settled(c, &d.pair_id).await?;
    }
    let state = State { root, devices };
    save(&state)?;
    serde_json::to_string_pretty(&state).map_err(|e| e.to_string())
}

/// Until the pair's first run has ended well.
async fn settled(c: &Client, pair_id: &str) -> R<()> {
    let end = Instant::now() + Duration::from_secs(30);
    loop {
        let s = c.status().await.map_err(|e| e.to_string())?;
        if s.pairs.iter().any(|p| p.pair.id == pair_id && idle(p)) {
            return Ok(());
        }
        if Instant::now() > end {
            return Err(format!("the first sync of {pair_id} did not end"));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Not running, started from its preview, and its last run succeeded.
fn idle(p: &wasl_ipc::PairState) -> bool {
    !p.running && !p.pair.status.hold_for_preview && p.pair.status.last_status == Some(RunStatus::Success)
}

fn start(wasld: &Path, root: &Path, name: &str) -> R<Device> {
    let dir = root.join(name);
    let (home, folder, log) = (dir.join("home"), dir.join("folder"), dir.join("wasld.log"));
    for d in [&home, &folder] {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    let out = std::fs::File::create(&log).map_err(|e| e.to_string())?;
    let err = out.try_clone().map_err(|e| e.to_string())?;
    let mut cmd = Command::new(wasld);
    cmd.arg("--home")
        .arg(&home)
        .env("WASL_MIQYAS_NAME", format!("Miqyas {}", name.to_uppercase()))
        .env_remove("WASL_MIQYAS_LISTEN")
        .stdin(Stdio::null())
        .stdout(out)
        .stderr(err);
    // Its own process group: a Ctrl-C meant for the caller leaves it for `down`.
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    let child = cmd
        .spawn()
        .map_err(|e| format!("cannot start {}: {e}", wasld.display()))?;
    Ok(Device {
        name: name.to_uppercase(),
        home,
        folder,
        pid: child.id(),
        log,
        device_id: String::new(),
        listen: String::new(),
        pair_id: PAIR_ID.into(),
    })
}

/// The daemon's client once it answers.
async fn ready(d: &Device) -> R<Client> {
    let paths = Paths::at(&d.home);
    let until = Instant::now() + Duration::from_secs(30);
    loop {
        if let Ok(c) = Client::connect(&paths).await {
            return Ok(c);
        }
        if !alive(d.pid) || Instant::now() > until {
            let log = std::fs::read_to_string(&d.log).unwrap_or_default();
            return Err(format!("device {} did not start: {}", d.name, log.trim()));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// B joins A's group with A's code; both confirm once their four words match.
async fn pair(a: &Client, b: &Client) -> R<()> {
    let e = |e: wasl_ipc::ClientError| e.to_string();
    let code = a
        .invite_device()
        .await
        .map_err(e)?
        .code
        .ok_or("no pairing code")?;
    b.join_device(&code).await.map_err(e)?;
    let (sa, sb) = (
        until(a, PairingStage::Confirm).await?,
        until(b, PairingStage::Confirm).await?,
    );
    if sa.words.len() != 4 || sa.words != sb.words {
        let _ = a.confirm_pairing(false).await;
        let _ = b.confirm_pairing(false).await;
        return Err(format!("the words differ: {:?} and {:?}", sa.words, sb.words));
    }
    a.confirm_pairing(true).await.map_err(e)?;
    b.confirm_pairing(true).await.map_err(e)?;
    until(a, PairingStage::Paired).await?;
    until(b, PairingStage::Paired).await?;
    Ok(())
}

async fn until(c: &Client, stage: PairingStage) -> R<PairingStatus> {
    let end = Instant::now() + Duration::from_secs(30);
    loop {
        let s = c.pairing_status().await.map_err(|e| e.to_string())?;
        if s.stage == stage {
            return Ok(s);
        }
        if s.stage == PairingStage::Failed || Instant::now() > end {
            return Err(format!(
                "pairing stopped at {:?}: {}",
                s.stage,
                s.message.unwrap_or_default()
            ));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// A makes the pair and offers it to B; B accepts it at its folder. Returns B's pair id.
async fn share(a: &Client, b: &Client, da: &Device, db: &Device) -> R<String> {
    let e = |e: wasl_ipc::ClientError| e.to_string();
    let mut p = Pair::new(PAIR_ID, "Miqyas", &da.folder.display().to_string());
    p.route = Route::Devices;
    p.reach = Reach::Lan;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64);
    p.created_at = now;
    p.updated_at = now;
    a.create_pair(p).await.map_err(e)?;
    let share = a.share_pair(PAIR_ID, None).await.map_err(e)?;
    a.share_with(PAIR_ID, false, vec![db.device_id.clone()])
        .await
        .map_err(e)?;
    let end = Instant::now() + Duration::from_secs(30);
    loop {
        let offers = b.offers().await.map_err(e)?;
        if offers
            .iter()
            .any(|o| o.share == share && o.from_device == da.device_id)
        {
            break;
        }
        if Instant::now() > end {
            return Err("B never saw A's offer".into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let pb = b
        .accept_offer(&share, &da.device_id, &db.folder.display().to_string(), None)
        .await
        .map_err(e)?;
    Ok(pb.id)
}

fn load(root: &Path) -> R<State> {
    let bytes = std::fs::read(root.join(STATE)).map_err(|e| format!("{}: {e}", root.display()))?;
    serde_json::from_slice(&bytes).map_err(|e| e.to_string())
}

fn save(state: &State) -> R<()> {
    let bytes = serde_json::to_vec_pretty(state).map_err(|e| e.to_string())?;
    std::fs::write(state.root.join(STATE), bytes).map_err(|e| e.to_string())
}

#[derive(Serialize)]
struct Waited {
    in_sync: bool,
    elapsed_ms: u128,
    files: usize,
    bytes: u64,
}

/// Until both folders hold the same files (by content) and neither device runs a sync.
async fn wait(root: &Path, timeout: Duration) -> R<String> {
    let state = load(root)?;
    let started = Instant::now();
    let mut clients = Vec::new();
    for d in &state.devices {
        clients.push(ready(d).await?);
    }
    loop {
        let mut all_idle = true;
        for (d, c) in state.devices.iter().zip(&clients) {
            let s = c.status().await.map_err(|e| e.to_string())?;
            let p = s.pairs.iter().find(|p| p.pair.id == d.pair_id);
            all_idle &= p.is_some_and(idle);
        }
        let trees: Vec<_> = state.devices.iter().map(|d| tree(&d.folder)).collect();
        if all_idle && trees[0] == trees[1] && same_content(&state) {
            let files = trees[0].len();
            let bytes = trees[0].iter().map(|(_, len)| len).sum();
            let w = Waited {
                in_sync: true,
                elapsed_ms: started.elapsed().as_millis(),
                files,
                bytes,
            };
            return serde_json::to_string(&w).map_err(|e| e.to_string());
        }
        if started.elapsed() > timeout {
            return Err(format!(
                "not in sync after {} s (files: {})",
                timeout.as_secs(),
                trees
                    .iter()
                    .map(|t| t.len().to_string())
                    .collect::<Vec<_>>()
                    .join(" and ")
            ));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Every file under `dir` (relative path, size), sorted. Names starting with a dot are left out,
/// as Wasl leaves them out by default.
fn tree(dir: &Path) -> Vec<(String, u64)> {
    fn walk(base: &Path, dir: &Path, out: &mut Vec<(String, u64)>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            if e.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            let path = e.path();
            match e.file_type() {
                Ok(t) if t.is_dir() => walk(base, &path, out),
                Ok(t) if t.is_file() => {
                    let rel = path
                        .strip_prefix(base)
                        .unwrap_or(&path)
                        .to_string_lossy()
                        .replace('\\', "/");
                    out.push((rel, e.metadata().map_or(0, |m| m.len())));
                }
                _ => {}
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    out
}

/// The files listed alike in both folders hold the same bytes.
fn same_content(state: &State) -> bool {
    let [a, b] = [&state.devices[0].folder, &state.devices[1].folder];
    tree(a)
        .iter()
        .all(|(rel, _)| std::fs::read(a.join(rel)).ok() == std::fs::read(b.join(rel)).ok())
}

#[derive(Serialize)]
struct Stopped {
    stopped: Vec<u32>,
    removed: PathBuf,
}

/// Stops both daemons (asking first, then killing) and removes the root `up` made.
async fn down(root: &Path) -> R<String> {
    if !root.join(MARKER).exists() {
        return Err(format!(
            "{} was not made by wasl-miqyas up; nothing removed",
            root.display()
        ));
    }
    let devices = load(root).map(|s| s.devices).unwrap_or_default();
    for d in &devices {
        if let Ok(Ok(c)) =
            tokio::time::timeout(Duration::from_secs(2), Client::connect(&Paths::at(&d.home))).await
        {
            let _ = c.shutdown().await;
        }
    }
    for d in &devices {
        let end = Instant::now() + Duration::from_secs(10);
        while alive(d.pid) && Instant::now() < end {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        if alive(d.pid) {
            kill(d.pid);
        }
    }
    std::fs::remove_dir_all(root).map_err(|e| format!("cannot remove {}: {e}", root.display()))?;
    let s = Stopped {
        stopped: devices.iter().map(|d| d.pid).collect(),
        removed: root.to_path_buf(),
    };
    serde_json::to_string(&s).map_err(|e| e.to_string())
}

#[cfg(unix)]
fn alive(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

#[cfg(unix)]
fn kill(pid: u32) {
    let _ = Command::new("kill").args(["-9", &pid.to_string()]).status();
}

#[cfg(windows)]
fn alive(pid: u32) -> bool {
    Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH"])
        .output()
        .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).contains(&pid.to_string()))
}

#[cfg(windows)]
fn kill(pid: u32) {
    let _ = Command::new("taskkill")
        .args(["/F", "/PID", &pid.to_string()])
        .status();
}
