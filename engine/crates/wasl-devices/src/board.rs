//! The group files in the Drive app-data folder (docs/DEVICE_SYNC.md §3, docs/SECURITY.md S2-1):
//! each device writes the group operations it holds to its own file and reads its members' files,
//! so devices that are never online together still learn of each other's additions and removals.
//! A device removed on one computer is cut off by a phone that never meets that computer, as soon
//! as the phone reads the computer's file.
//!
//! The files are read at start and every [`BoardTiming::read_every`], and this device's file is
//! written whenever its group changes (checked every [`BoardTiming::check_every`]), and read back
//! at every read: anyone holding the account's token (a removed device included) can overwrite or
//! delete it to keep the news of its removal from devices that rely on Drive, so a file that is
//! not what this device would write is written again ([`restore_own`]). What is read
//! goes through [`Devices::receive_ops`], so it is checked exactly like operations a member sent
//! over a link. Whether Drive is used at all is [`crate::ReachPolicy::board`]'s decision.

use std::sync::Arc;
use std::time::{Duration, Instant};

use wasl_net::rendezvous::{self, Rendezvous, RendezvousError};

use crate::devices::Devices;

/// How often the group files are looked at.
#[derive(Debug, Clone, Copy)]
pub struct BoardTiming {
    /// How often this device checks whether its own file needs writing.
    pub check_every: Duration,
    /// How often the members' files are read (also once at start).
    pub read_every: Duration,
}

impl Default for BoardTiming {
    fn default() -> Self {
        Self {
            check_every: Duration::from_secs(5),
            read_every: Duration::from_secs(60),
        }
    }
}

/// Writes this device's group file: every operation it holds, the newest that fit.
pub async fn publish(devices: &Devices, rv: &dyn Rendezvous) -> Result<(), RendezvousError> {
    rendezvous::publish_group(rv, devices.genesis(), devices.id(), &devices.group_ops()).await
}

/// Reads this device's own group file back and writes it again when it is not exactly what this
/// device would write (changed, emptied or deleted by someone else). True if it was written.
pub async fn restore_own(devices: &Devices, rv: &dyn Rendezvous) -> Result<bool, RendezvousError> {
    let want = rendezvous::group_file(&devices.group_ops(), rendezvous::MAX_GROUP_FILE_BYTES);
    let name = rendezvous::group_file_name(&devices.genesis(), &devices.id());
    if rv.get(&name).await?.as_deref() == Some(want.as_slice()) {
        return Ok(false);
    }
    rv.put(&name, want).await?;
    Ok(true)
}

/// Reads the members' group files and takes in what counts; true if the members changed. Each
/// file is taken in before the next is chosen, so a member added in one file is read in the same
/// round only if it was already a member; it is read at the next one.
pub async fn read(devices: &Devices, rv: &dyn Rendezvous) -> Result<bool, String> {
    let members: Vec<wasl_net::DeviceId> = devices.members().iter().map(|m| m.id).collect();
    let files = rendezvous::fetch_group(rv, devices.genesis(), devices.id(), &members)
        .await
        .map_err(|e| e.to_string())?;
    let mut changed = false;
    for (_, ops) in files {
        changed |= devices.receive_ops(&ops)?;
    }
    Ok(changed)
}

/// Runs while the daemon runs. `board` gives the store to use now (`None`: no account, or no pair
/// lets the group go to Drive), checked every round so a change of policy takes effect.
pub async fn run(
    devices: Arc<Devices>,
    board: impl Fn() -> Option<Arc<dyn Rendezvous>> + Send + Sync + 'static,
    timing: BoardTiming,
) {
    // The store last used (a new account starts over), the heads last written there, and when
    // the members' files were last read.
    let mut store: Option<usize> = None;
    let mut written: Option<Vec<wasl_net::OpId>> = None;
    let mut read_at: Option<Instant> = None;
    loop {
        match board() {
            None => {
                (store, written, read_at) = (None, None, None);
            }
            Some(rv) => {
                let this = Arc::as_ptr(&rv) as *const () as usize;
                if store != Some(this) {
                    (store, written, read_at) = (Some(this), None, None);
                }
                // Only a member's file is read by anyone; a removed device stops writing.
                let member = || devices.members().iter().any(|m| m.id == devices.id());
                if read_at.is_none_or(|t| t.elapsed() >= timing.read_every)
                    && read(&devices, rv.as_ref()).await.is_ok()
                {
                    read_at = Some(Instant::now());
                    // One more read a round: this device's own file, put back if someone else
                    // changed or deleted it.
                    let heads = devices.group_heads();
                    if member() && restore_own(&devices, rv.as_ref()).await.is_ok() {
                        written = Some(heads);
                    }
                }
                let heads = devices.group_heads();
                let member = member();
                if member && written.as_ref() != Some(&heads) && publish(&devices, rv.as_ref()).await.is_ok()
                {
                    written = Some(heads);
                }
            }
        }
        tokio::time::sleep(timing.check_every).await;
    }
}
