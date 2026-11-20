//! A harness for tests that run real `wasld` processes (test builds, feature `miqyas`): each
//! device gets its own home and synced folder, runs on loopback with a log file, and is killed
//! when the test ends, passes or fails. Reused by the mixed-version test, which starts a daemon
//! from another binary.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use wasl_core::{Reach, Route};
use wasl_ipc::{Client, PairingStage, PairingStatus, Paths};
use wasl_store::Pair;

/// The daemon built from this tree (with the `miqyas` feature the tests run under).
pub const CURRENT: &str = env!("CARGO_BIN_EXE_wasld");
/// How long any one wait lasts before the test fails with the daemon logs.
pub const WAIT: Duration = Duration::from_secs(60);
const PAIR_ID: &str = "two-daemons";

/// One `wasld` process and the folders it owns.
pub struct Device {
    pub name: String,
    pub home: PathBuf,
    pub folder: PathBuf,
    pub log: PathBuf,
    bin: String,
    child: Child,
}

fn spawn(bin: &str, home: &Path, log: &Path, name: &str) -> Child {
    // Appended, so a restarted device keeps the log of its first run.
    let out = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)
        .unwrap();
    let err = out.try_clone().unwrap();
    Command::new(bin)
        .arg("--home")
        .arg(home)
        .env("WASL_MIQYAS_NAME", name)
        .env_remove("WASL_MIQYAS_LISTEN")
        .stdin(Stdio::null())
        .stdout(out)
        .stderr(err)
        .spawn()
        .unwrap_or_else(|e| panic!("cannot start {bin}: {e}"))
}

impl Device {
    /// Starts the daemon `bin` for `name` under `root`, and waits until it answers.
    pub async fn start(bin: &str, root: &Path, name: &str) -> Device {
        let dir = root.join(name);
        let (home, folder, log) = (dir.join("home"), dir.join("folder"), dir.join("wasld.log"));
        for d in [&home, &folder] {
            std::fs::create_dir_all(d).unwrap();
        }
        let child = spawn(bin, &home, &log, name);
        let mut device = Device {
            name: name.into(),
            home,
            folder,
            log,
            bin: bin.into(),
            child,
        };
        device.wait_ready().await;
        device
    }

    /// Stops the daemon the hard way, as a crash or a closed laptop would. Its home stays.
    pub fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    /// Stops the daemon and starts it again on the same home, and waits until it answers.
    pub async fn restart(&mut self) {
        self.stop();
        self.child = spawn(&self.bin, &self.home, &self.log, &self.name);
        self.wait_ready().await;
    }

    async fn wait_ready(&mut self) {
        let end = Instant::now() + WAIT;
        loop {
            if Client::connect(&Paths::at(&self.home)).await.is_ok() {
                return;
            }
            if let Ok(Some(status)) = self.child.try_wait() {
                panic!(
                    "device {} exited ({status}) before it answered:\n{}",
                    self.name,
                    self.logs()
                );
            }
            if Instant::now() > end {
                panic!(
                    "device {} did not answer in {WAIT:?}:\n{}",
                    self.name,
                    self.logs()
                );
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    pub async fn client(&self) -> Client {
        Client::connect(&Paths::at(&self.home))
            .await
            .unwrap_or_else(|e| panic!("device {}: {e}\n{}", self.name, self.logs()))
    }

    pub fn logs(&self) -> String {
        let text = std::fs::read_to_string(&self.log).unwrap_or_default();
        let lines: Vec<&str> = text.lines().collect();
        let tail = &lines[lines.len().saturating_sub(40)..];
        format!("--- log of {} ---\n{}", self.name, tail.join("\n"))
    }

    /// Asks the daemon to sync every pair it has now (a run already going is left alone).
    pub async fn sync_now(&self) {
        // A sync call returns when the run ends, so the whole round is bounded.
        let round = async {
            let Ok(c) = Client::connect(&Paths::at(&self.home)).await else {
                return;
            };
            let Ok(s) = c.status().await else { return };
            for p in s.pairs {
                let _ = c.sync(&p.pair.id, wasl_engine::Trigger::Manual).await;
            }
        };
        let _ = tokio::time::timeout(Duration::from_secs(20), round).await;
    }

    /// What the daemon says about each of its pairs, for failure messages.
    pub async fn pair_states(&self) -> String {
        let Ok(c) = Client::connect(&Paths::at(&self.home)).await else {
            return format!("(device {} does not answer)", self.name);
        };
        match c.status().await {
            Ok(s) => s
                .pairs
                .iter()
                .map(|p| {
                    format!(
                        "pair {} on {}: running={} hold={} last={:?} error={:?}",
                        p.pair.id,
                        self.name,
                        p.running,
                        p.pair.status.hold_for_preview,
                        p.pair.status.last_status,
                        p.pair.status.last_error
                    )
                })
                .collect::<Vec<_>>()
                .join("\n"),
            Err(e) => format!("(status of {}: {e})", self.name),
        }
    }

    pub fn path(&self, rel: &str) -> PathBuf {
        self.folder.join(rel)
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Polls `check` until it returns `Some`, or fails the test naming `what` and printing the logs.
pub async fn eventually<T, F, Fut>(what: &str, devices: &[&Device], mut check: F) -> T
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Option<T>>,
{
    let end = Instant::now() + WAIT;
    loop {
        // A call inside `check` that never answers must not outlive the deadline.
        let left = end.saturating_duration_since(Instant::now());
        if let Ok(Some(v)) = tokio::time::timeout(left, check()).await {
            return v;
        }
        if Instant::now() > end {
            let mut logs = Vec::new();
            for d in devices {
                logs.push(format!("{}\n{}", d.logs(), d.pair_states().await));
            }
            panic!("timed out after {WAIT:?} waiting for {what}\n{}", logs.join("\n"));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

pub async fn at_stage(c: &Client, d: &Device, stage: PairingStage) -> PairingStatus {
    eventually(
        &format!("{} to reach pairing stage {stage:?}", d.name),
        &[d],
        || async {
            let s = c.pairing_status().await.ok()?;
            assert_ne!(
                PairingStage::Failed,
                s.stage,
                "pairing failed: {:?}
{}",
                s.message,
                d.logs()
            );
            (s.stage == stage).then_some(s)
        },
    )
    .await
}

/// B joins the group of A with the invitation A made. Returns the four words each device showed.
pub async fn pair(a: &Device, b: &Device) -> (Vec<String>, Vec<String>) {
    let (ca, cb) = (a.client().await, b.client().await);
    let code = ca
        .invite_device()
        .await
        .unwrap()
        .code
        .expect("the invitation carries a code");
    cb.join_device(&code).await.unwrap();
    let wa = at_stage(&ca, a, PairingStage::Confirm).await.words;
    let wb = at_stage(&cb, b, PairingStage::Confirm).await.words;
    if wa == wb {
        ca.confirm_pairing(true).await.unwrap();
        cb.confirm_pairing(true).await.unwrap();
        at_stage(&ca, a, PairingStage::Paired).await;
        at_stage(&cb, b, PairingStage::Paired).await;
        // Loopback has no discovery: each device is told where the other listens.
        let la = ca.devices().await.unwrap().listen.expect("A listens");
        let lb = cb.devices().await.unwrap().listen.expect("B listens");
        ca.connect_device(&lb).await.unwrap();
        cb.connect_device(&la).await.unwrap();
    } else {
        let _ = ca.confirm_pairing(false).await;
        let _ = cb.confirm_pairing(false).await;
    }
    (wa, wb)
}

/// The id of the device itself.
pub async fn id_of(d: &Device) -> String {
    let info = d.client().await.devices().await.unwrap();
    info.devices
        .into_iter()
        .find(|x| x.this_device)
        .expect("a device list holds this device")
        .id
}

/// A makes a folder pair, shares it with B, and B accepts it: both folders sync both ways.
pub async fn share_folder(a: &Device, b: &Device) {
    let (ca, cb) = (a.client().await, b.client().await);
    let (ida, idb) = (id_of(a).await, id_of(b).await);
    let mut p = Pair::new(PAIR_ID, "Two daemons", &a.folder.display().to_string());
    p.route = Route::Devices;
    p.reach = Reach::Lan;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64);
    p.created_at = now;
    p.updated_at = now;
    ca.create_pair(p).await.unwrap();
    let share = ca.share_pair(PAIR_ID, None).await.unwrap();
    ca.share_with(PAIR_ID, false, vec![idb]).await.unwrap();
    eventually("the offer to reach the second device", &[a, b], || async {
        let offers = cb.offers().await.ok()?;
        offers
            .iter()
            .any(|o| o.share == share && o.from_device == ida)
            .then_some(())
    })
    .await;
    let pb = cb
        .accept_offer(&share, &ida, &b.folder.display().to_string(), None)
        .await
        .unwrap();
    // Starting from the preview is the go-ahead for a new pair's first sync.
    for (c, d, id) in [(&ca, a, PAIR_ID), (&cb, b, pb.id.as_str())] {
        let run = c.sync(id, wasl_engine::Trigger::Preview);
        match tokio::time::timeout(WAIT, run).await {
            Ok(r) => {
                r.unwrap();
            }
            Err(_) => panic!(
                "the first sync on {} did not end in {WAIT:?}
{}",
                d.name,
                d.logs()
            ),
        }
    }
    for (c, d, id) in [(&ca, a, PAIR_ID), (&cb, b, pb.id.as_str())] {
        eventually(
            &format!("the first sync of {} to end well", d.name),
            &[a, b],
            || async {
                let s = c.status().await.ok()?;
                let p = s.pairs.iter().find(|p| p.pair.id == id)?;
                let idle = !p.running
                    && !p.pair.status.hold_for_preview
                    && p.pair.status.last_status == Some(wasl_core::RunStatus::Success);
                idle.then_some(())
            },
        )
        .await;
    }
}

/// Waits until `rel` in the folder of `d` holds `content` (or, with `None`, is gone).
pub async fn expect_file(d: &Device, all: &[&Device], rel: &str, content: Option<&str>) {
    expect_file_with(d, all, rel, content, true).await
}

/// Like [`expect_file`]; with `nudge` false nothing asks the daemons to sync, so only their own
/// folder watchers and schedules can bring the change over.
pub async fn expect_file_with(d: &Device, all: &[&Device], rel: &str, content: Option<&str>, nudge: bool) {
    let want = content.map(|c| c.as_bytes().to_vec());
    eventually(&format!("{rel} to be {content:?} on {}", d.name), all, || async {
        if std::fs::read(d.path(rel)).ok() == want {
            return Some(());
        }
        // The watcher and its settle time are not what these tests are about: ask both devices
        // to look now, so the wait is for the sync itself.
        if nudge {
            for dev in all {
                dev.sync_now().await;
            }
            tokio::time::sleep(Duration::from_millis(400)).await;
        }
        None
    })
    .await;
}
