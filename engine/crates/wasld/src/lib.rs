//! `wasld`, the Wasl desktop daemon (SRS FR-B2, FR-E4): it owns the engine and the database for
//! this OS user, runs pairs on schedule and on change, and answers the `wasl` CLI and the tray app
//! over `wasl-ipc`.

pub mod daemon;
pub mod disk;
pub mod env;
mod journal;
#[cfg(feature = "miqyas")]
pub mod miqyas;
pub mod oauth;
pub mod provider;
pub mod schedule;
pub mod tokens;
pub mod vaults;
pub mod views;

use daemon::{Daemon, DaemonConfig, Ipc};
use std::sync::Arc;
use wasl_ipc::{Paths, Server};

/// Opens the engine, binds the socket and serves until a client asks it to stop.
pub async fn serve(paths: &Paths, cfg: DaemonConfig, timing: schedule::Timing) -> Result<(), String> {
    let daemon = Daemon::open(cfg)?;
    daemon.keep_out_of_pairs(paths.runtime.clone());
    let server = Server::bind(paths, Arc::new(Ipc(daemon.clone())), env!("CARGO_PKG_VERSION"))
        .map_err(|e| format!("Cannot open the local socket: {e}"))?;
    daemon.attach_events(server.events());
    let serving = tokio::spawn(server.serve());
    let scheduling = tokio::spawn(schedule::run(daemon.clone(), timing));
    // Read-only views: mounted now, refreshed on their schedule.
    let viewing = tokio::spawn(views::run(daemon.views.clone(), timing.tick));
    // Agent undo: journals changes in watched folders while a session is open (#57).
    let journaling = tokio::spawn(journal::run(daemon.clone(), timing.tick));
    // macOS: the File Provider extension's own socket, in the app group's container, answering
    // its requests only (docs/ON_DEMAND.md §2.6). Without it the extension can show nothing.
    let providing = daemon.views.provider().and_then(|host| {
        // Content left by an earlier run (a crash before the system took it) goes first.
        host.forget_all_content();
        let at = host.paths();
        match Server::bind(
            &at,
            Arc::new(provider::ProviderIpc(host)),
            env!("CARGO_PKG_VERSION"),
        ) {
            Ok(s) => Some(tokio::spawn(s.serve())),
            Err(e) => {
                eprintln!(
                    "wasld: the File Provider socket can't be opened in {}: {e}",
                    at.runtime.display()
                );
                None
            }
        }
    });
    // Devices beyond the LAN, when a pair asks for it, and the group files in Drive's app-data
    // folder for members that are never online together (docs/SECURITY.md S2-1).
    let reaching = daemon.devices.clone().map(|d| {
        let policy = Arc::new(wasl_devices::ReachPolicy::new(
            daemon.store.clone(),
            daemon.client.clone(),
            paths.data.join("tmp"),
        ));
        let board = policy.clone();
        let boarding = tokio::spawn(wasl_devices::board::run(
            d.clone(),
            move || board.board(),
            wasl_devices::BoardTiming::default(),
        ));
        let reaching = tokio::spawn(wasl_devices::reach::run(
            d,
            move || policy.current(),
            stun_servers(),
            wasl_devices::ReachTiming::default(),
        ));
        [reaching, boarding]
    });
    // A device said a pair changed: run it.
    let notified = daemon.take_notifications().map(|mut rx| {
        let d = daemon.clone();
        tokio::spawn(async move {
            while let Some(pair_id) = rx.recv().await {
                let d = d.clone();
                tokio::spawn(async move {
                    d.run(&pair_id, wasl_engine::Trigger::Instant).await;
                });
            }
        })
    });
    daemon.shutdown.notified().await;
    // Let the last answers and the ShuttingDown event reach the clients.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    scheduling.abort();
    viewing.abort();
    journaling.abort();
    // Views disconnect; their folders stay set up for the next start.
    daemon.views.close_all().await;
    serving.abort();
    if let Some(p) = providing {
        p.abort();
    }
    if let Some(n) = notified {
        n.abort();
    }
    for r in reaching.into_iter().flatten() {
        r.abort();
    }
    if let Some(d) = &daemon.devices {
        d.close();
    }
    Ok(())
}

/// Public STUN servers for reach beyond the LAN; none in the Miqyas test build, which stays on
/// loopback.
fn stun_servers() -> Vec<String> {
    if cfg!(feature = "miqyas") {
        return Vec::new();
    }
    wasl_net::stun::DEFAULT_SERVERS
        .iter()
        .map(|s| (*s).to_owned())
        .collect()
}
