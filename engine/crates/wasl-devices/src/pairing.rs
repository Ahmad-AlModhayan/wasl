//! Pairing a new device (docs/DEVICE_SYNC.md §2) over the network.
//!
//! The inviter opens pairing on its device endpoint (while the code is open any key may connect,
//! and may do nothing but pair) and shows a code naming this device and where to reach it. The joiner connects with
//! the inviter's device ID pinned, and the two run [`wasl_net::Session`]s in rounds: the joiner
//! sends its messages, the inviter answers with its own. Both people see four words and confirm
//! on both devices; then the inviter adds the joiner to the group and hands it the group's
//! operations, which the joiner adopts.
//!
//! The joiner's key in the pairing messages must be the key it proved in the TLS handshake, so a
//! relay in the middle cannot pair its own key. One code, one attempt: the first connection uses
//! the invitation up, whatever happens next.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::status::{PairingStage, PairingStatus};
use async_trait::async_trait;
use wasl_net::pairing::{Invitation, JoinCode, PairingCode, Session, State};
use wasl_net::{
    DeviceId, Endpoint, IndexPage, PairReply, PairRound, PeerError, PeerLink, PeerResult, PeerService,
    ShareId, Trust,
};

use crate::devices::{Devices, clean_name};

/// How often the joiner asks the inviter for news.
const POLL: Duration = Duration::from_millis(300);

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn idle() -> PairingStatus {
    PairingStatus {
        stage: PairingStage::Idle,
        inviting: false,
        code: None,
        expires_at: None,
        words: Vec::new(),
        peer_name: None,
        peer_id: None,
        confirmed_here: false,
        message: None,
    }
}

/// The one pairing this daemon runs at a time, and what the screens show about it.
pub struct Pairing {
    devices: Arc<Devices>,
    device_name: String,
    status: Arc<Mutex<PairingStatus>>,
    /// The inviter's or joiner's session.
    session: Arc<Mutex<Option<Session>>>,
    /// Messages produced here (the local confirmation) waiting for the next round.
    outbox: Arc<Mutex<Vec<wasl_net::pairing::Message>>>,
    on_change: Arc<dyn Fn() + Send + Sync>,
}

impl Pairing {
    pub fn new(devices: Arc<Devices>, device_name: &str, on_change: Arc<dyn Fn() + Send + Sync>) -> Self {
        Self {
            devices,
            device_name: device_name.to_owned(),
            status: Arc::new(Mutex::new(idle())),
            session: Arc::new(Mutex::new(None)),
            outbox: Arc::new(Mutex::new(Vec::new())),
            on_change,
        }
    }

    pub fn status(&self) -> PairingStatus {
        self.status.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    fn busy(&self) -> bool {
        matches!(self.status().stage, PairingStage::Waiting | PairingStage::Confirm)
    }

    /// Opens an invitation: a pairing endpoint on every interface, and the code to show.
    pub fn invite(&self) -> Result<PairingStatus, String> {
        if self.busy() {
            return Err("A pairing is already open. Finish or cancel it first.".into());
        }
        let bound = self.devices.local_addr().map_err(|e| e.to_string())?;
        // Listening everywhere: every LAN address; listening on one address: that one.
        let addrs = if bound.ip().is_unspecified() {
            wasl_net::holepunch::local_addrs(bound.port(), wasl_core::Reach::Lan).lan
        } else {
            vec![bound]
        };
        if addrs.is_empty() {
            return Err("This computer has no network address other devices could reach.".into());
        }
        let invitation = Invitation::new(self.devices.id(), addrs, now()).map_err(|e| e.to_string())?;
        let code = invitation.code().to_qr();
        let expires_at = invitation.expires_at();
        let host: Arc<dyn PeerService> = Arc::new(Host {
            invitation: Mutex::new(Some(invitation)),
            session: self.session.clone(),
            outbox: self.outbox.clone(),
            status: self.status.clone(),
            devices: self.devices.clone(),
            peer: Mutex::new(None),
            on_change: self.on_change.clone(),
        });
        *self.session.lock().unwrap_or_else(|p| p.into_inner()) = None;
        self.outbox.lock().unwrap_or_else(|p| p.into_inner()).clear();
        self.devices.open_pairing(host.clone());
        // The code stops working at its expiry whatever else happens.
        let (devices, status, change) = (self.devices.clone(), self.status.clone(), self.on_change.clone());
        let wait = Duration::from_secs(expires_at.saturating_sub(now()));
        tokio::spawn(async move {
            tokio::time::sleep(wait).await;
            if devices.pairing_host_is(&host) {
                devices.close_pairing();
                let mut st = status.lock().unwrap_or_else(|p| p.into_inner());
                if matches!(st.stage, PairingStage::Waiting | PairingStage::Confirm) {
                    st.stage = PairingStage::Failed;
                    st.code = None;
                    st.message = Some("The pairing code expired. Make a new one.".into());
                }
                drop(st);
                change();
            }
        });
        let status = PairingStatus {
            stage: PairingStage::Waiting,
            inviting: true,
            code: Some(code),
            expires_at: Some(expires_at),
            ..idle()
        };
        *self.status.lock().unwrap_or_else(|p| p.into_inner()) = status.clone();
        (self.on_change)();
        Ok(status)
    }

    /// Joins the device whose code was scanned or pasted. Runs in the background; follow it with
    /// [`Pairing::status`].
    pub fn join(self: &Arc<Self>, code: &str) -> Result<(), String> {
        if self.busy() {
            return Err("A pairing is already open. Finish or cancel it first.".into());
        }
        let code =
            PairingCode::from_qr(code.trim()).map_err(|_| "That is not a Wasl pairing code.".to_string())?;
        if code.device_id == self.devices.id() {
            return Err("That code was made on this computer.".into());
        }
        let (session, hello) = Session::join(&JoinCode::Qr(code.clone()), self.devices.public_key(), now())
            .map_err(|e| e.to_string())?;
        *self.session.lock().unwrap_or_else(|p| p.into_inner()) = Some(session);
        {
            let mut out = self.outbox.lock().unwrap_or_else(|p| p.into_inner());
            out.clear();
            out.push(hello);
        }
        self.set(PairingStatus {
            stage: PairingStage::Waiting,
            inviting: false,
            peer_id: Some(code.device_id.to_string()),
            ..idle()
        });
        let me = self.clone();
        tokio::spawn(async move {
            let result = me.run_join(code).await;
            if let Err(message) = result {
                me.fail(message);
            }
        });
        Ok(())
    }

    async fn run_join(&self, code: PairingCode) -> Result<(), String> {
        // Only the device the code names may answer.
        let inviter = code.device_id;
        let trust: Trust = Arc::new(move |pk| DeviceId::from_public_key(pk) == inviter);
        let endpoint = Endpoint::bind(SocketAddr::from(([0, 0, 0, 0], 0)), self.devices.key(), trust)
            .map_err(|e| e.to_string())?;
        let mut link = None;
        for addr in &code.addrs {
            if let Ok(l) = endpoint.connect(*addr, Some(inviter)).await {
                link = Some(l);
                break;
            }
        }
        let link = link.ok_or("Couldn't reach the other device. Check that both are on the same network.")?;
        let deadline = now() + wasl_net::pairing::CODE_TTL_SECS;
        loop {
            let msgs = std::mem::take(&mut *self.outbox.lock().unwrap_or_else(|p| p.into_inner()));
            let reply = link
                .pair(PairRound {
                    msgs,
                    name: Some(self.device_name.clone()),
                })
                .await
                .map_err(|e| format!("The other device stopped answering: {e}"))?;
            let state = {
                let mut guard = self.session.lock().unwrap_or_else(|p| p.into_inner());
                let s = guard.as_mut().ok_or("Pairing was cancelled.")?;
                let mut out = Vec::new();
                for m in &reply.msgs {
                    out.extend(s.handle(m, now()));
                }
                out.extend(s.tick(now()));
                self.outbox.lock().unwrap_or_else(|p| p.into_inner()).extend(out);
                self.show(s);
                s.state()
            };
            match state {
                State::Paired if !reply.ops.is_empty() => {
                    let inviter_pk = {
                        let guard = self.session.lock().unwrap_or_else(|p| p.into_inner());
                        guard.as_ref().and_then(|s| s.paired_peer()).map(|p| p.pk)
                    }
                    .ok_or("Pairing ended without a peer.")?;
                    self.devices.join(&reply.ops, &inviter_pk)?;
                    let _ = self.devices.connect(link.remote_addr()).await;
                    let name = self
                        .devices
                        .members()
                        .into_iter()
                        .find(|d| d.id == inviter)
                        .map(|d| d.name);
                    self.set(PairingStatus {
                        stage: PairingStage::Paired,
                        peer_name: name,
                        peer_id: Some(inviter.to_string()),
                        ..idle()
                    });
                    link.close();
                    endpoint.close();
                    return Ok(());
                }
                State::Aborted(reason) => return Err(reason_text(reason)),
                _ => {}
            }
            if now() > deadline {
                return Err("The pairing code expired.".into());
            }
            tokio::time::sleep(POLL).await;
        }
    }

    /// The person here says the words match (`true`) or not.
    pub fn confirm(&self, accept: bool) -> Result<PairingStatus, String> {
        let mut guard = self.session.lock().unwrap_or_else(|p| p.into_inner());
        let s = guard.as_mut().ok_or("No pairing is waiting for you.")?;
        let msgs = if accept { s.confirm(now()) } else { s.reject() };
        self.outbox.lock().unwrap_or_else(|p| p.into_inner()).extend(msgs);
        if accept {
            self.status
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .confirmed_here = true;
            self.show(s);
        }
        drop(guard);
        if !accept {
            self.fail("You said the words don't match. Nothing was added.".into());
        }
        (self.on_change)();
        Ok(self.status())
    }

    /// Closes any open invitation or join.
    pub fn cancel(&self) {
        if let Some(s) = self.session.lock().unwrap_or_else(|p| p.into_inner()).as_mut() {
            let abort = s.reject();
            self.outbox
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .extend(abort);
        }
        self.devices.close_pairing();
        self.set(idle());
    }

    fn show(&self, s: &Session) {
        show(&self.status, s);
        (self.on_change)();
    }

    fn set(&self, status: PairingStatus) {
        *self.status.lock().unwrap_or_else(|p| p.into_inner()) = status;
        (self.on_change)();
    }

    fn fail(&self, message: String) {
        *self.session.lock().unwrap_or_else(|p| p.into_inner()) = None;
        self.devices.close_pairing();
        let inviting = self.status().inviting;
        self.set(PairingStatus {
            stage: PairingStage::Failed,
            inviting,
            message: Some(message),
            ..idle()
        });
    }
}

/// The words, once both proofs checked out.
fn show(status: &Mutex<PairingStatus>, s: &Session) {
    let mut st = status.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(words) = s.words() {
        st.words = words.iter().map(|w| (*w).to_owned()).collect();
    }
    match s.state() {
        State::AwaitingConfirm { .. } => st.stage = PairingStage::Confirm,
        State::Aborted(reason) => {
            st.stage = PairingStage::Failed;
            st.message = Some(reason_text(reason));
        }
        _ => {}
    }
}

fn reason_text(reason: wasl_net::pairing::AbortReason) -> String {
    use wasl_net::pairing::AbortReason::*;
    match reason {
        Expired => "The pairing code expired. Make a new one.",
        Timeout => "The other device took too long to answer.",
        ProofFailed | WrongDevice => "The code didn't match. Make a new one and try again.",
        Rejected | PeerAborted => "Pairing was cancelled on one of the devices.",
        _ => "Pairing failed. Make a new code and try again.",
    }
    .to_owned()
}

/// The inviter's side: answers pairing rounds on the pairing endpoint, nothing else.
struct Host {
    invitation: Mutex<Option<Invitation>>,
    session: Arc<Mutex<Option<Session>>>,
    outbox: Arc<Mutex<Vec<wasl_net::pairing::Message>>>,
    status: Arc<Mutex<PairingStatus>>,
    devices: Arc<Devices>,
    /// The joiner this attempt belongs to: the device of the first connection.
    peer: Mutex<Option<(DeviceId, Option<String>)>>,
    on_change: Arc<dyn Fn() + Send + Sync>,
}

#[async_trait]
impl PeerService for Host {
    async fn shared(&self, _peer: DeviceId) -> Vec<ShareId> {
        Vec::new()
    }

    async fn changes_since(&self, _: DeviceId, _: ShareId, _: u64, _: usize) -> PeerResult<IndexPage> {
        Err(PeerError::NotShared)
    }

    async fn chunk_list(
        &self,
        _: DeviceId,
        _: ShareId,
        _: &str,
        _: wasl_chunk::Hash,
    ) -> PeerResult<wasl_chunk::ChunkList> {
        Err(PeerError::NotShared)
    }

    async fn chunk(&self, _: DeviceId, _: ShareId, _: wasl_chunk::Hash) -> PeerResult<Vec<u8>> {
        Err(PeerError::NotShared)
    }

    async fn notify(&self, _: DeviceId, _: ShareId, _: u64) {}

    async fn pair(&self, peer: DeviceId, round: PairRound) -> PeerResult<PairReply> {
        let refuse = || PeerError::Protocol("this pairing belongs to another device".into());
        // The key in the joiner's Hello must be the key it proved to TLS.
        let mut hello = false;
        for m in &round.msgs {
            if let wasl_net::pairing::Message::Hello { pk, .. } = m {
                if DeviceId::from_public_key(pk) != peer {
                    return Err(refuse());
                }
                hello = true;
            }
        }
        {
            let mut p = self.peer.lock().unwrap_or_else(|e| e.into_inner());
            match p.as_ref() {
                // The invitation goes to the first connection that opens with a proper Hello;
                // anything else is refused without using it up.
                None if !hello => return Err(refuse()),
                None => {
                    // The joiner's name is the one in its first round, kept from then on: it
                    // cannot change what the person here sees after the words are shown
                    // (security review 2026-10).
                    let name = round.name.as_deref().and_then(clean_name);
                    *p = Some((peer, name.clone()));
                    let mut st = self.status.lock().unwrap_or_else(|e| e.into_inner());
                    st.peer_id = Some(peer.to_string());
                    st.peer_name = name;
                }
                Some((id, _)) if *id != peer => return Err(refuse()),
                Some(_) => {}
            }
        }
        let (msgs, paired, ended) = {
            let mut guard = self.session.lock().unwrap_or_else(|e| e.into_inner());
            if guard.is_none() {
                let mut inv = self.invitation.lock().unwrap_or_else(|e| e.into_inner());
                let started = match inv
                    .as_mut()
                    .ok_or_else(refuse)?
                    .start(self.devices.public_key(), now())
                {
                    Ok(s) => s,
                    Err(e) => {
                        drop(inv);
                        drop(guard);
                        self.devices.close_pairing();
                        return Err(PeerError::Remote(e.to_string()));
                    }
                };
                *guard = Some(started);
            }
            let s = guard.as_mut().expect("started");
            let mut out = std::mem::take(&mut *self.outbox.lock().unwrap_or_else(|e| e.into_inner()));
            for m in &round.msgs {
                out.extend(s.handle(m, now()));
            }
            out.extend(s.tick(now()));
            show(&self.status, s);
            let paired = matches!(s.state(), State::Paired)
                .then(|| s.paired_peer())
                .flatten()
                .filter(|p| p.id == peer);
            (out, paired, matches!(s.state(), State::Aborted(_)))
        };
        if ended {
            // Over without pairing: nobody else may connect on this code.
            self.devices.close_pairing();
        }
        (self.on_change)();
        let Some(paired) = paired else {
            return Ok(PairReply {
                msgs,
                ops: Vec::new(),
            });
        };
        // Both confirmed: add the joiner (once) and hand over the group.
        let name = self
            .peer
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .and_then(|(_, n)| n.clone())
            .unwrap_or_else(|| "New device".into());
        if !self.devices.members().iter().any(|d| d.id == paired.id) {
            self.devices
                .add_member(paired.pk, &name, now())
                .map_err(PeerError::Remote)?;
        }
        self.devices.close_pairing();
        {
            let mut st = self.status.lock().unwrap_or_else(|e| e.into_inner());
            st.stage = PairingStage::Paired;
            st.peer_name = Some(name);
            st.code = None;
        }
        (self.on_change)();
        Ok(PairReply {
            msgs,
            ops: self.devices.group_ops(),
        })
    }
}
