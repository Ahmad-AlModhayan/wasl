//! Tells subscribed clients when a folder's status changes (issue #29). The daemon decides every
//! folder's status once (`Daemon::folder_statuses`); this task listens to the events the daemon
//! already sends (a change, a finished run, progress), decides again, and sends
//! [`Event::FolderStatus`] for each folder whose status is not what it last sent. Statuses are sent in
//! English, the catalogue's own language: the server words them again for each client's language.

use crate::daemon::Daemon;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;
use wasl_ipc::Event;
use wasl_proto::{FolderStatus, Lang};

/// Progress comes many times a second; statuses are decided at most this often.
const SETTLE: Duration = Duration::from_millis(120);

pub async fn run(daemon: Arc<Daemon>, tx: broadcast::Sender<Event>) {
    let mut rx = tx.subscribe();
    let mut sent: HashMap<String, FolderStatus> = HashMap::new();
    loop {
        match rx.recv().await {
            // Its own events, and the end.
            Ok(Event::FolderStatus { .. }) => continue,
            Ok(Event::ShuttingDown) | Err(broadcast::error::RecvError::Closed) => return,
            Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
        }
        // One decision for a burst of events.
        tokio::time::sleep(SETTLE).await;
        while rx.try_recv().is_ok() {}
        let Ok(now) = daemon.folder_statuses(Lang::En) else {
            continue;
        };
        sent.retain(|id, _| now.iter().any(|s| &s.pair_id == id));
        for status in now {
            if sent.get(&status.pair_id) != Some(&status) {
                sent.insert(status.pair_id.clone(), status.clone());
                let _ = tx.send(Event::FolderStatus {
                    pair_id: status.pair_id.clone(),
                    status: Box::new(status),
                });
            }
        }
    }
}
