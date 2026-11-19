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

use std::collections::{HashSet, VecDeque};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::status::{PairingStage, PairingStatus};
use async_trait::async_trait;
use wasl_net::pairing::{Invitation, JoinCode, PairingCode, Session, ShortCode, State};
use wasl_net::{
    DeviceId, Endpoint, IndexPage, PairReply, PairRound, PeerError, PeerLink, PeerResult, PeerService,
    ShareId, Trust,
};

use crate::devices::{Devices, clean_name};

/// How often the joiner asks the inviter for news.
const POLL: Duration = Duration::from_millis(300);
/// How long a joiner holding a typed code looks for a device with a pairing open.
const FIND_WAIT: Duration = Duration::from_secs(20);

/// The catalogue words of a pairing failure, in English: what `PairingStatus::message` carries.
fn words_en(key: &str) -> String {
    use wasl_proto::words::{Lang, Values, render};
    render(key, &Values::default(), Lang::En, None, &[], &mut |_| {}).text
}

/// Why a join stopped.
enum Stop {
    /// The person cancelled: the status is already idle.
    Cancelled,
    /// Ended with the catalogue entry `key`.
    Failed(&'static str),
}

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
        short_code: None,
        expires_at: None,
        words: Vec::new(),
        peer_name: None,
        peer_id: None,
        confirmed_here: false,
        message: None,
        failure: None,
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
        let mut invitation = Invitation::new(self.devices.id(), addrs, now()).map_err(|e| e.to_string())?;
        // The same invitation also reads as 8 characters a person can type.
        let short = invitation.enable_short_code().to_string();
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
                    st.short_code = None;
                    st.failure = Some("pair.code_expired".into());
                    st.message = Some(words_en("pair.code_expired"));
                }
                drop(st);
                change();
            }
        });
        let status = PairingStatus {
            stage: PairingStage::Waiting,
            inviting: true,
            code: Some(code),
            short_code: Some(short),
            expires_at: Some(expires_at),
            ..idle()
        };
        *self.status.lock().unwrap_or_else(|p| p.into_inner()) = status.clone();
        (self.on_change)();
        Ok(status)
    }

    /// Joins the device whose code was scanned or typed, with no address to try first
    /// ([`Pairing::join_at`]).
    pub fn join(self: &Arc<Self>, code: &str) -> Result<(), String> {
        self.join_at(code, &[])
    }

    /// Joins the device whose code was scanned (the long `WASL:` code) or typed (8 characters,
    /// any case, with or without a dash or spaces). Runs in the background; follow it with
    /// [`Pairing::status`], where a code that can't be read, or doesn't match, ends as a
    /// `Failed` pairing with a catalogue key.
    ///
    /// `hints` are addresses (`ip:port`) to try for a typed code, for a network where
    /// discovery is blocked. They only say where to knock: the code still has to prove itself.
    pub fn join_at(self: &Arc<Self>, code: &str, hints: &[String]) -> Result<(), String> {
        if self.busy() {
            return Err("A pairing is already open. Finish or cancel it first.".into());
        }
        let hints = hints
            .iter()
            .map(|h| {
                h.trim()
                    .parse::<SocketAddr>()
                    .map_err(|_| format!("{h:?} is not an address like 192.168.1.5:4242."))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let code = code.trim();
        let plan = if code.get(..5).is_some_and(|p| p.eq_ignore_ascii_case("WASL:")) {
            match PairingCode::from_qr(code) {
                Ok(c) => Plan::Qr(c),
                Err(_) => {
                    self.fail("pair.bad_code");
                    return Ok(());
                }
            }
        } else {
            match ShortCode::parse(code) {
                Ok(c) => Plan::Short(c, hints),
                Err(_) => {
                    self.fail("pair.bad_code");
                    return Ok(());
                }
            }
        };
        if let Plan::Qr(c) = &plan
            && c.device_id == self.devices.id()
        {
            return Err("That code was made on this computer.".into());
        }
        let peer_id = match &plan {
            Plan::Qr(c) => Some(c.device_id.to_string()),
            Plan::Short(..) => None,
        };
        *self.session.lock().unwrap_or_else(|p| p.into_inner()) = None;
        self.outbox.lock().unwrap_or_else(|p| p.into_inner()).clear();
        self.set(PairingStatus {
            stage: PairingStage::Waiting,
            inviting: false,
            peer_id,
            ..idle()
        });
        let me = self.clone();
        tokio::spawn(async move {
            match me.run_join(plan).await {
                Ok(()) | Err(Stop::Cancelled) => {}
                Err(Stop::Failed(key)) => me.fail(key),
            }
        });
        Ok(())
    }

    /// Starts this device's side with `code` and queues the Hello to send first.
    fn start_session(&self, code: &JoinCode) -> Result<(), Stop> {
        let (session, hello) =
            Session::join(code, self.devices.public_key(), now()).map_err(|_| Stop::Failed("pair.other"))?;
        *self.session.lock().unwrap_or_else(|p| p.into_inner()) = Some(session);
        let mut out = self.outbox.lock().unwrap_or_else(|p| p.into_inner());
        out.clear();
        out.push(hello);
        Ok(())
    }

    async fn run_join(&self, plan: Plan) -> Result<(), Stop> {
        match plan {
            Plan::Qr(code) => {
                self.start_session(&JoinCode::Qr(code.clone()))?;
                // Only the device the code names may answer.
                let inviter = code.device_id;
                let trust: Trust = Arc::new(move |pk| DeviceId::from_public_key(pk) == inviter);
                let endpoint = Endpoint::bind(SocketAddr::from(([0, 0, 0, 0], 0)), self.devices.key(), trust)
                    .map_err(|_| Stop::Failed("pair.other"))?;
                let result = self.try_inviter(&endpoint, &code.addrs, Some(inviter)).await;
                endpoint.close();
                result.map_err(|e| e.unwrap_or(Stop::Failed("pair.not_found")))
            }
            Plan::Short(code, hints) => self.run_short(code, hints).await,
        }
    }

    /// A typed code does not say who made it: try the hints, then every device announcing an open
    /// pairing, each with a fresh session. A device that answers wrongly costs only its own
    /// invitation, and nothing the joiner sends before the inviter proves itself depends on the
    /// code.
    async fn run_short(&self, code: ShortCode, hints: Vec<SocketAddr>) -> Result<(), Stop> {
        let endpoint = Endpoint::bind(
            SocketAddr::from(([0, 0, 0, 0], 0)),
            self.devices.key(),
            Arc::new(|_| true),
        )
        .map_err(|_| Stop::Failed("pair.other"))?;
        let mut browser = self.devices.browse_pairing();
        let mut end = tokio::time::Instant::now() + FIND_WAIT;
        let mut queue: VecDeque<Vec<SocketAddr>> = hints.into_iter().map(|h| vec![h]).collect();
        let mut seen: HashSet<SocketAddr> = HashSet::new();
        let mut answered: Option<&'static str> = None;
        let result = loop {
            if !self.busy() {
                break Err(Stop::Cancelled);
            }
            let Some(addrs) = queue.pop_front() else {
                let Some(b) = browser.as_mut() else {
                    break Err(Stop::Failed(answered.unwrap_or("pair.not_found")));
                };
                match tokio::time::timeout_at(end, b.next()).await {
                    Ok(Some(found)) if found.token != self.devices.token() => {
                        let fresh: Vec<_> = found.addrs.into_iter().filter(|a| seen.insert(*a)).collect();
                        if !fresh.is_empty() {
                            queue.push_back(fresh);
                        }
                        continue;
                    }
                    Ok(Some(_)) => continue,
                    _ => break Err(Stop::Failed(answered.unwrap_or("pair.not_found"))),
                }
            };
            if let Err(stop) = self.start_session(&JoinCode::Short(code.clone())) {
                break Err(stop);
            }
            match self.try_inviter(&endpoint, &addrs, None).await {
                Ok(()) => break Ok(()),
                // Nobody answered there: look on.
                Err(None) => {}
                // Refused before anything was used up, and for good: no other device matters.
                Err(Some(Stop::Failed("pair.device_removed"))) => {
                    break Err(Stop::Failed("pair.device_removed"));
                }
                Err(Some(Stop::Cancelled)) => break Err(Stop::Cancelled),
                // A device answered and the code did not fit it: the next may be the right one.
                Err(Some(Stop::Failed(key))) => {
                    answered.get_or_insert(key);
                    // Someone is there: a late second announcement gets a moment, not the
                    // whole wait.
                    end = end.min(tokio::time::Instant::now() + Duration::from_millis(1500));
                }
            }
        };
        endpoint.close();
        result
    }

    /// Reaches the inviter at the first of `addrs` that answers and runs the rounds to the end.
    /// `Err(None)`: no address answered at all, so nothing was used up anywhere.
    async fn try_inviter(
        &self,
        endpoint: &Endpoint,
        addrs: &[SocketAddr],
        pinned: Option<DeviceId>,
    ) -> Result<(), Option<Stop>> {
        let mut link = None;
        for addr in addrs {
            if let Ok(l) = endpoint.connect(*addr, pinned).await {
                link = Some(l);
                break;
            }
        }
        let Some(link) = link else { return Err(None) };
        let result = self.rounds(&link).await;
        link.close();
        result.map_err(Some)
    }

    async fn rounds(&self, link: &wasl_net::QuicLink) -> Result<(), Stop> {
        let inviter = link.peer();
        let deadline = now() + wasl_net::pairing::CODE_TTL_SECS;
        loop {
            let msgs = std::mem::take(&mut *self.outbox.lock().unwrap_or_else(|p| p.into_inner()));
            let reply = match link
                .pair(PairRound {
                    msgs,
                    name: Some(self.device_name.clone()),
                })
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    let key = remote_key(&e);
                    if key == "pair.device_removed" && self.devices.members().iter().any(|m| m.id == inviter)
                    {
                        // A member of this device's own group says its key was removed: the
                        // next start gives it a new key and a clean group.
                        let _ = self.devices.note_removed();
                    } else if key == "pair.device_removed" {
                        return Err(Stop::Failed("pair.other"));
                    }
                    return Err(Stop::Failed(key));
                }
            };
            // The inviter's key in its Hello must be the key it proved to TLS.
            let swapped = reply.msgs.iter().any(|m| {
                matches!(m, wasl_net::pairing::Message::Hello { pk, .. } if DeviceId::from_public_key(pk) != inviter)
            });
            if swapped {
                return Err(Stop::Failed("pair.other"));
            }
            let state = {
                let mut guard = self.session.lock().unwrap_or_else(|p| p.into_inner());
                let Some(s) = guard.as_mut() else {
                    return Err(Stop::Cancelled);
                };
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
                    .ok_or(Stop::Failed("pair.other"))?;
                    self.devices
                        .join(&reply.ops, &inviter_pk)
                        .map_err(|_| Stop::Failed("pair.other"))?;
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
                    return Ok(());
                }
                State::Aborted(reason) => {
                    // Tell the inviter (best effort), so its screen does not wait on a dead code.
                    let msgs = std::mem::take(&mut *self.outbox.lock().unwrap_or_else(|p| p.into_inner()));
                    if !msgs.is_empty() {
                        let _ = link.pair(PairRound { msgs, name: None }).await;
                    }
                    return Err(Stop::Failed(abort_key(reason)));
                }
                _ => {}
            }
            if now() > deadline {
                return Err(Stop::Failed("pair.code_expired"));
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
            self.fail_with(
                "pair.cancelled",
                "You said the words don't match. Nothing was added.".into(),
            );
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

    /// Ends the pairing with the catalogue entry `key`.
    fn fail(&self, key: &'static str) {
        self.fail_with(key, words_en(key));
    }

    fn fail_with(&self, key: &'static str, message: String) {
        *self.session.lock().unwrap_or_else(|p| p.into_inner()) = None;
        self.devices.close_pairing();
        let inviting = self.status().inviting;
        self.set(PairingStatus {
            stage: PairingStage::Failed,
            inviting,
            message: Some(message),
            failure: Some(key.to_owned()),
            ..idle()
        });
    }
}

/// What a join starts from.
enum Plan {
    /// A scanned code: names the inviter and where it is.
    Qr(PairingCode),
    /// A typed code, and addresses to try before looking on the network.
    Short(ShortCode, Vec<SocketAddr>),
}

/// What the inviter says when it refuses a device the group removed. Checked by text on the
/// joiner, which is all a refusal carries.
const REMOVED: &str = "device was removed";

/// The catalogue entry for a failure the inviter answered a round with.
fn remote_key(e: &PeerError) -> &'static str {
    let text = e.to_string();
    if text.contains(REMOVED) {
        "pair.device_removed"
    } else if text.contains("expired") {
        "pair.code_expired"
    } else if text.contains("already used") {
        "pair.code_used"
    } else {
        "pair.other"
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
            let key = abort_key(reason);
            st.stage = PairingStage::Failed;
            st.message = Some(words_en(key));
            st.failure = Some(key.to_owned());
        }
        _ => {}
    }
}

fn abort_key(reason: wasl_net::pairing::AbortReason) -> &'static str {
    use wasl_net::pairing::AbortReason::*;
    match reason {
        Expired => "pair.code_expired",
        ProofFailed | WrongDevice => "pair.wrong_code",
        Rejected | PeerAborted => "pair.cancelled",
        _ => "pair.other",
    }
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
                // A device the group removed never pairs on its old key. Refused here, before the
                // invitation is started, so the code stays good for the device it was made for;
                // what the group already knows is all this uses, no new proof.
                None if self.devices.group_removed(&peer) => {
                    return Err(PeerError::Remote(format!("this {REMOVED} from the group")));
                }
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
            st.short_code = None;
        }
        (self.on_change)();
        Ok(PairReply {
            msgs,
            ops: self.devices.group_ops(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wasl_net::pairing::AbortReason;
    use wasl_proto::words::entry;

    #[test]
    fn every_pairing_failure_has_a_catalogue_entry_in_both_languages() {
        for key in [
            "pair.bad_code",
            "pair.wrong_code",
            "pair.code_expired",
            "pair.code_used",
            "pair.not_found",
            "pair.device_removed",
            "pair.cancelled",
            "pair.other",
        ] {
            let e = entry(key).unwrap_or_else(|| panic!("{key} is not in the catalogue"));
            assert!(!e.en.other.is_empty() && !e.ar.other.is_empty(), "{key}");
            assert_eq!(e.en.other, words_en(key));
        }
    }

    #[test]
    fn the_other_devices_answers_map_to_typed_failures() {
        let remote = |t: &str| remote_key(&PeerError::Remote(t.into()));
        assert_eq!("pair.code_expired", remote("this pairing code has expired"));
        assert_eq!("pair.code_used", remote("this pairing code was already used"));
        assert_eq!(
            "pair.device_removed",
            remote(&format!("this {REMOVED} from the group"))
        );
        assert_eq!("pair.other", remote("disk on fire"));
        assert_eq!("pair.wrong_code", abort_key(AbortReason::ProofFailed));
        assert_eq!("pair.code_expired", abort_key(AbortReason::Expired));
        assert_eq!("pair.cancelled", abort_key(AbortReason::PeerAborted));
        assert_eq!("pair.other", abort_key(AbortReason::Timeout));
    }
}
