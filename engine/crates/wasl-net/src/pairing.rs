//! Pairing (docs/DEVICE_SYNC.md §2): adding a new device to a person's group with a one-time
//! code, as a pure state machine. The transport (QUIC/TLS, later) carries [`Message`]s between
//! two [`Session`]s and feeds the clock in; nothing here touches the network.
//!
//! # Codes
//!
//! The existing device (the **inviter**) creates an [`Invitation`]: a random 20-byte secret,
//! single use, valid for [`CODE_TTL_SECS`]. It can be shown two ways:
//!
//! - **QR** ([`PairingCode::to_qr`]): `WASL:` + base32 of
//!   `version(1) ‖ device ID(16) ‖ secret(20) ‖ n(1) ‖ n × (4|6, IP, port) ‖ BLAKE3 check(4)`.
//!   It names the inviter (its public key must hash to the ID) and where to reach it, and carries
//!   the whole 160-bit secret.
//! - **Short code** ([`ShortCode`], `XXXX-XXXX`): 8 symbols = the first 40 bits of the secret,
//!   nothing else. All 40 bits go to authentication, none to addressing.
//!
//! **How a short code finds the inviter.** While a short code is on screen, the inviter
//! announces "pairing open" on the LAN (the `discover` module: a random per-boot token, nothing
//! derived from the code) and, when both devices use the same Google account, writes a
//! rendezvous file in the Drive app-data folder. The joiner connects to each candidate and runs
//! the protocol below with the typed code. Because the joiner reveals nothing that depends on
//! the code until the candidate has proved it knows the code, trying a wrong candidate costs
//! nothing and leaks nothing.
//!
//! **Why a 40-bit code is enough here.** Without a PAKE, whoever receives a valid proof can test
//! guesses offline, so the short-code design rests on four rules:
//!
//! 1. *The inviter proves first.* A fake inviter must send a proof before learning anything,
//!    so it cannot get the joiner to reveal anything code-dependent. A fake joiner does receive
//!    the inviter's proof, but it must answer within [`PROOF_TIMEOUT_SECS`] and —
//! 2. *the invitation is single use:* the first connection ([`Invitation::start`]) consumes it,
//!    whatever happens next. A stranger who sees nothing wins one online guess at 2⁻⁴⁰, then
//!    the code is dead.
//! 3. *The key is stretched* with PBKDF2-HMAC-SHA256 ([`SHORT_CODE_ROUNDS`] rounds), salted with
//!    both public keys: testing all 2⁴⁰ codes against one captured proof costs about 2⁵⁹ SHA-256
//!    compressions, far beyond the proof timeout and the 10-minute life of the joiner's session,
//!    so even a captured proof cannot be turned into an impersonation in time.
//! 4. *The four words* (44 bits, over both public keys) must match on both screens and be
//!    confirmed on both before either side adds the other.
//!
//! A short code is accepted only by an invitation whose short code was shown
//! ([`Invitation::enable_short_code`]); a QR-only invitation keeps its full 160 bits. A typo in a
//! short code makes the joiner reject the inviter's proof and abort, which uses up the
//! invitation: the person shows a new code. A device that sends junk to an open invitation can
//! only use it up (denial of service), never pair.
//!
//! # Protocol
//!
//! ```text
//! joiner  → inviter   Hello { pk_J, nonce_J, kind }
//! inviter → joiner    Hello { pk_I, nonce_I, kind }, Proof { mac_I }
//! joiner  → inviter   Proof { mac_J }        (only after mac_I checks out)
//! both                show four words; wait for the local person and the peer's Confirm
//! either  → other     Confirm | Abort
//! ```
//!
//! `mac_X = HMAC-SHA256(key, "wasl-pair" ‖ kind ‖ role_X ‖ pk_X ‖ pk_peer ‖ nonce_I ‖ nonce_J)`
//! where `role` is `I` or `J`: a proof reflected back to its sender, or replayed into another
//! session (fresh nonces), fails. Proofs are compared in constant time. `key` is the 20-byte
//! secret for a QR code, or `PBKDF2(short code, "wasl-pair-short" ‖ pk_I ‖ pk_J)` for a short
//! code. The words are `BLAKE3("wasl-words" ‖ min(pk_I, pk_J) ‖ max(pk_I, pk_J) ‖ key)`, four
//! 11-bit indexes into [`crate::words::WORDS`].

use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use hmac::{Hmac, Mac};
use sha2::Sha256;
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

use crate::base32;
use crate::identity::{DeviceId, parse_public_key};
use crate::words::WORDS;

type HmacSha256 = Hmac<Sha256>;

/// Length of the one-time secret.
pub const SECRET_LEN: usize = 20;
/// Bytes of the secret carried by a short code (40 bits).
pub const SHORT_CODE_BYTES: usize = 5;
/// Symbols in a short code.
pub const SHORT_CODE_SYMBOLS: usize = 8;
/// How long an invitation (and a joiner's session) lives.
pub const CODE_TTL_SECS: u64 = 600;
/// How long a side waits for the peer's next protocol message.
pub const PROOF_TIMEOUT_SECS: u64 = 30;
/// PBKDF2 rounds stretching a short code into a key.
pub const SHORT_CODE_ROUNDS: u32 = 200_000;
/// Most addresses a QR code carries.
pub const MAX_ADDRS: usize = 8;

const QR_PREFIX: &str = "WASL:";
const QR_VERSION: u8 = 1;
const QR_CHECK_LEN: usize = 4;
const QR_MIN_BYTES: usize = 1 + 16 + SECRET_LEN + 1 + QR_CHECK_LEN;
const QR_MAX_BYTES: usize = QR_MIN_BYTES + MAX_ADDRS * (1 + 16 + 2);

/// A code that could not be read.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CodeError {
    #[error("not a Wasl pairing code")]
    Malformed,
    #[error("pairing code version {0} is not supported")]
    Version(u8),
}

/// An invitation that cannot start a session.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PairError {
    #[error("this pairing code was already used")]
    Used,
    #[error("this pairing code has expired")]
    Expired,
    #[error("the system random generator failed")]
    Rng,
}

fn random<const N: usize>() -> Result<[u8; N], PairError> {
    let mut b = [0u8; N];
    getrandom::fill(&mut b).map_err(|_| PairError::Rng)?;
    Ok(b)
}

/// What the inviter shows: its ID, the one-time secret and where to reach it.
#[derive(Clone, PartialEq, Eq, Zeroize, ZeroizeOnDrop)]
pub struct PairingCode {
    #[zeroize(skip)]
    pub device_id: DeviceId,
    pub secret: [u8; SECRET_LEN],
    #[zeroize(skip)]
    pub addrs: Vec<SocketAddr>,
}

impl fmt::Debug for PairingCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PairingCode")
            .field("device_id", &self.device_id)
            .field("secret", &"<redacted>")
            .field("addrs", &self.addrs)
            .finish()
    }
}

impl PairingCode {
    /// The QR payload text (QR alphanumeric mode: upper case, digits and `:`). At most
    /// [`MAX_ADDRS`] addresses are written.
    pub fn to_qr(&self) -> String {
        let addrs = &self.addrs[..self.addrs.len().min(MAX_ADDRS)];
        let mut b = Zeroizing::new(Vec::with_capacity(QR_MAX_BYTES));
        b.push(QR_VERSION);
        b.extend_from_slice(self.device_id.as_bytes());
        b.extend_from_slice(&self.secret);
        b.push(addrs.len() as u8);
        for a in addrs {
            match a.ip() {
                IpAddr::V4(ip) => {
                    b.push(4);
                    b.extend_from_slice(&ip.octets());
                }
                IpAddr::V6(ip) => {
                    b.push(6);
                    b.extend_from_slice(&ip.octets());
                }
            }
            b.extend_from_slice(&a.port().to_be_bytes());
        }
        let check = qr_check(&b);
        b.extend_from_slice(&check);
        format!("{QR_PREFIX}{}", base32::encode(&b))
    }

    /// Reads a QR payload strictly: exact prefix, upper-case symbols, zero padding, valid check,
    /// known version, no trailing bytes.
    pub fn from_qr(text: &str) -> Result<Self, CodeError> {
        let body = text.strip_prefix(QR_PREFIX).ok_or(CodeError::Malformed)?;
        let n_bytes = body.len() * 5 / 8;
        if !(QR_MIN_BYTES..=QR_MAX_BYTES).contains(&n_bytes) {
            return Err(CodeError::Malformed);
        }
        let b = Zeroizing::new(base32::decode_strict(body, n_bytes).ok_or(CodeError::Malformed)?);
        let (payload, check) = b.split_at(b.len() - QR_CHECK_LEN);
        if qr_check(payload) != check {
            return Err(CodeError::Malformed);
        }
        if payload[0] != QR_VERSION {
            return Err(CodeError::Version(payload[0]));
        }
        let mut r = Reader(&payload[1..]);
        let device_id = DeviceId::from_bytes(r.array()?);
        let secret = r.array::<SECRET_LEN>()?;
        let n = usize::from(r.array::<1>()?[0]);
        if n > MAX_ADDRS {
            return Err(CodeError::Malformed);
        }
        let mut addrs = Vec::with_capacity(n);
        for _ in 0..n {
            let ip = match r.array::<1>()?[0] {
                4 => IpAddr::V4(Ipv4Addr::from(r.array::<4>()?)),
                6 => IpAddr::V6(Ipv6Addr::from(r.array::<16>()?)),
                _ => return Err(CodeError::Malformed),
            };
            let port = u16::from_be_bytes(r.array()?);
            addrs.push(SocketAddr::new(ip, port));
        }
        if !r.0.is_empty() {
            return Err(CodeError::Malformed);
        }
        Ok(Self {
            device_id,
            secret,
            addrs,
        })
    }

    /// The short code for this secret: its first 40 bits.
    pub fn short_code(&self) -> ShortCode {
        let mut b = [0u8; SHORT_CODE_BYTES];
        b.copy_from_slice(&self.secret[..SHORT_CODE_BYTES]);
        ShortCode(b)
    }
}

fn qr_check(payload: &[u8]) -> [u8; QR_CHECK_LEN] {
    let mut h = blake3::Hasher::new();
    h.update(b"wasl-qr");
    h.update(payload);
    let mut out = [0u8; QR_CHECK_LEN];
    out.copy_from_slice(&h.finalize().as_bytes()[..QR_CHECK_LEN]);
    out
}

struct Reader<'a>(&'a [u8]);

impl Reader<'_> {
    fn array<const N: usize>(&mut self) -> Result<[u8; N], CodeError> {
        let (head, tail) = self.0.split_at_checked(N).ok_or(CodeError::Malformed)?;
        self.0 = tail;
        let mut out = [0u8; N];
        out.copy_from_slice(head);
        Ok(out)
    }
}

/// The 8-symbol code a person types: the first 40 bits of the invitation's secret.
#[derive(Clone, PartialEq, Eq, Zeroize, ZeroizeOnDrop)]
pub struct ShortCode([u8; SHORT_CODE_BYTES]);

impl ShortCode {
    /// Reads a typed code: any case, `-` and spaces ignored, look-alikes folded; exactly 8
    /// symbols.
    pub fn parse(text: &str) -> Result<Self, CodeError> {
        let values = Zeroizing::new(base32::read_typed(text).ok_or(CodeError::Malformed)?);
        if values.len() != SHORT_CODE_SYMBOLS {
            return Err(CodeError::Malformed);
        }
        let bytes =
            Zeroizing::new(base32::from_values(&values, SHORT_CODE_BYTES).ok_or(CodeError::Malformed)?);
        let mut b = [0u8; SHORT_CODE_BYTES];
        b.copy_from_slice(&bytes);
        Ok(Self(b))
    }
}

/// Shown as `XXXX-XXXX`.
impl fmt::Display for ShortCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = Zeroizing::new(base32::encode(&self.0));
        write!(f, "{}-{}", &text[..4], &text[4..])
    }
}

impl fmt::Debug for ShortCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ShortCode(<redacted>)")
    }
}

/// Which code the joiner used; bound into the key and every proof.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum CodeKind {
    Qr,
    Short,
}

impl CodeKind {
    fn byte(self) -> u8 {
        match self {
            CodeKind::Qr => 1,
            CodeKind::Short => 2,
        }
    }
}

/// The code the joiner has: scanned or typed.
#[derive(Clone, Debug)]
pub enum JoinCode {
    Qr(PairingCode),
    Short(ShortCode),
}

/// The side of a session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Inviter,
    Joiner,
}

impl Role {
    fn byte(self) -> u8 {
        match self {
            Role::Inviter => b'I',
            Role::Joiner => b'J',
        }
    }
}

/// A pairing protocol message.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Message {
    Hello {
        pk: [u8; 32],
        nonce: [u8; 32],
        kind: CodeKind,
    },
    Proof {
        mac: [u8; 32],
    },
    Confirm,
    Abort,
}

/// Why a session ended without pairing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AbortReason {
    /// The invitation (or the joiner's session) outlived [`CODE_TTL_SECS`].
    Expired,
    /// The peer did not answer within [`PROOF_TIMEOUT_SECS`].
    Timeout,
    /// The peer's Hello was unusable (bad key, our own key, wrong code kind).
    BadHello,
    /// The inviter's key does not match the device ID in the QR code.
    WrongDevice,
    /// A short code was used but this invitation never showed one.
    ShortCodeNotShown,
    /// The peer's proof was wrong: it does not know the code.
    ProofFailed,
    /// A message arrived out of order.
    Unexpected,
    /// The local person said the words do not match, or cancelled.
    Rejected,
    /// The peer sent Abort.
    PeerAborted,
}

/// Where a session stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// Waiting for the peer's Hello.
    AwaitingHello,
    /// Waiting for the peer's Proof.
    AwaitingProof,
    /// Both proofs checked; the words are shown and the session waits for the local person's
    /// confirmation and the peer's Confirm.
    AwaitingConfirm { local: bool, peer: bool },
    /// Both confirmed: the peer may be added to the group.
    Paired,
    /// Ended; nothing may be added.
    Aborted(AbortReason),
}

/// The peer a finished session vouches for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PairedPeer {
    pub pk: [u8; 32],
    pub id: DeviceId,
}

/// An open invitation on the inviter. Single use: one [`Invitation::start`] only.
pub struct Invitation {
    code: PairingCode,
    created_at: u64,
    short_shown: bool,
    used: bool,
}

impl Invitation {
    /// A new invitation with a random secret, created at `now` (Unix seconds).
    pub fn new(device_id: DeviceId, addrs: Vec<SocketAddr>, now: u64) -> Result<Self, PairError> {
        Ok(Self {
            code: PairingCode {
                device_id,
                secret: random()?,
                addrs,
            },
            created_at: now,
            short_shown: false,
            used: false,
        })
    }

    /// The code to show as a QR.
    pub fn code(&self) -> &PairingCode {
        &self.code
    }

    /// The short code to show; from now on this invitation also accepts it.
    pub fn enable_short_code(&mut self) -> ShortCode {
        self.short_shown = true;
        self.code.short_code()
    }

    /// When the invitation stops working.
    pub fn expires_at(&self) -> u64 {
        self.created_at.saturating_add(CODE_TTL_SECS)
    }

    /// Starts the one session this invitation allows, for an incoming connection. The
    /// invitation is used up even if the session then fails.
    pub fn start(&mut self, own_pk: [u8; 32], now: u64) -> Result<Session, PairError> {
        if self.used {
            return Err(PairError::Used);
        }
        if now >= self.expires_at() {
            return Err(PairError::Expired);
        }
        self.used = true;
        let secret = Zeroizing::new(self.code.secret.to_vec());
        // The secret now lives only in the session.
        self.code.secret.zeroize();
        let kinds = if self.short_shown {
            &[CodeKind::Qr, CodeKind::Short][..]
        } else {
            &[CodeKind::Qr][..]
        };
        let session = Session {
            role: Role::Inviter,
            accepted: kinds.to_vec(),
            kind: None,
            secret,
            expected_inviter: None,
            own_pk,
            own_nonce: random()?,
            peer: None,
            key: None,
            expires_at: self.expires_at(),
            deadline: now.saturating_add(PROOF_TIMEOUT_SECS),
            words: None,
            state: State::AwaitingHello,
        };
        Ok(session)
    }
}

/// One side of one pairing attempt.
pub struct Session {
    role: Role,
    /// Code kinds this side accepts (inviter: Qr, plus Short if shown; joiner: its own).
    accepted: Vec<CodeKind>,
    kind: Option<CodeKind>,
    /// The full secret (QR) or the 5 short-code bytes.
    secret: Zeroizing<Vec<u8>>,
    /// Joiner with a QR code: the inviter's ID from the code.
    expected_inviter: Option<DeviceId>,
    own_pk: [u8; 32],
    own_nonce: [u8; 32],
    /// The peer's (pk, nonce).
    peer: Option<([u8; 32], [u8; 32])>,
    key: Option<Zeroizing<Vec<u8>>>,
    expires_at: u64,
    deadline: u64,
    words: Option<[&'static str; 4]>,
    state: State,
}

impl Session {
    /// Starts the joiner's side; send the returned Hello to the candidate inviter.
    pub fn join(code: &JoinCode, own_pk: [u8; 32], now: u64) -> Result<(Self, Message), PairError> {
        let (kind, secret, expected_inviter) = match code {
            JoinCode::Qr(c) => (CodeKind::Qr, Zeroizing::new(c.secret.to_vec()), Some(c.device_id)),
            JoinCode::Short(s) => (CodeKind::Short, Zeroizing::new(s.0.to_vec()), None),
        };
        let session = Self {
            role: Role::Joiner,
            accepted: vec![kind],
            kind: Some(kind),
            secret,
            expected_inviter,
            own_pk,
            own_nonce: random()?,
            peer: None,
            key: None,
            expires_at: now.saturating_add(CODE_TTL_SECS),
            deadline: now.saturating_add(PROOF_TIMEOUT_SECS),
            words: None,
            state: State::AwaitingHello,
        };
        let hello = session.hello(kind);
        Ok((session, hello))
    }

    /// This side's role.
    pub fn role(&self) -> Role {
        self.role
    }

    /// Where the session stands.
    pub fn state(&self) -> State {
        self.state
    }

    /// The four words to show, once both proofs have checked out.
    pub fn words(&self) -> Option<[&'static str; 4]> {
        match self.state {
            State::AwaitingConfirm { .. } | State::Paired => self.words,
            _ => None,
        }
    }

    /// The peer to add to the group — only once both sides confirmed.
    pub fn paired_peer(&self) -> Option<PairedPeer> {
        match (self.state, self.peer) {
            (State::Paired, Some((pk, _))) => Some(PairedPeer {
                pk,
                id: DeviceId::from_public_key(&pk),
            }),
            _ => None,
        }
    }

    fn is_final(&self) -> bool {
        matches!(self.state, State::Paired | State::Aborted(_))
    }

    fn hello(&self, kind: CodeKind) -> Message {
        Message::Hello {
            pk: self.own_pk,
            nonce: self.own_nonce,
            kind,
        }
    }

    /// Ends the session; key material is wiped. Returns the Abort to send (none if the peer
    /// aborted).
    fn abort(&mut self, reason: AbortReason) -> Vec<Message> {
        self.state = State::Aborted(reason);
        self.wipe();
        if reason == AbortReason::PeerAborted {
            Vec::new()
        } else {
            vec![Message::Abort]
        }
    }

    fn wipe(&mut self) {
        self.secret.zeroize();
        self.key = None;
    }

    /// Applies the clock; returns an Abort to send if the session timed out.
    pub fn tick(&mut self, now: u64) -> Vec<Message> {
        if self.is_final() {
            return Vec::new();
        }
        if now >= self.expires_at {
            return self.abort(AbortReason::Expired);
        }
        if !matches!(self.state, State::AwaitingConfirm { .. }) && now > self.deadline {
            return self.abort(AbortReason::Timeout);
        }
        Vec::new()
    }

    /// Feeds a message from the peer; returns the messages to send back, in order.
    pub fn handle(&mut self, msg: &Message, now: u64) -> Vec<Message> {
        if self.is_final() {
            return Vec::new();
        }
        let out = self.tick(now);
        if self.is_final() {
            return out;
        }
        match (self.state, msg) {
            (_, Message::Abort) => self.abort(AbortReason::PeerAborted),
            (State::AwaitingHello, Message::Hello { pk, nonce, kind }) => {
                self.on_hello(*pk, *nonce, *kind, now)
            }
            (State::AwaitingProof, Message::Proof { mac }) => self.on_proof(mac),
            (State::AwaitingConfirm { local, .. }, Message::Confirm) => {
                self.state = State::AwaitingConfirm { local, peer: true };
                self.finish_if_confirmed();
                Vec::new()
            }
            _ => self.abort(AbortReason::Unexpected),
        }
    }

    /// The local person confirmed the words match. Returns the Confirm to send.
    pub fn confirm(&mut self, now: u64) -> Vec<Message> {
        let out = self.tick(now);
        if self.is_final() {
            return out;
        }
        match self.state {
            State::AwaitingConfirm { local: false, peer } => {
                self.state = State::AwaitingConfirm { local: true, peer };
                self.finish_if_confirmed();
                vec![Message::Confirm]
            }
            _ => Vec::new(),
        }
    }

    /// The local person said the words differ, or cancelled. Returns the Abort to send.
    pub fn reject(&mut self) -> Vec<Message> {
        if self.is_final() {
            return Vec::new();
        }
        self.abort(AbortReason::Rejected)
    }

    fn finish_if_confirmed(&mut self) {
        if self.state
            == (State::AwaitingConfirm {
                local: true,
                peer: true,
            })
        {
            self.state = State::Paired;
            self.wipe();
        }
    }

    fn on_hello(&mut self, pk: [u8; 32], nonce: [u8; 32], kind: CodeKind, now: u64) -> Vec<Message> {
        if pk == self.own_pk || parse_public_key(&pk).is_err() || nonce == self.own_nonce {
            return self.abort(AbortReason::BadHello);
        }
        if !self.accepted.contains(&kind) {
            return self.abort(match (self.role, kind) {
                (Role::Inviter, CodeKind::Short) => AbortReason::ShortCodeNotShown,
                _ => AbortReason::BadHello,
            });
        }
        if self
            .expected_inviter
            .is_some_and(|expected| DeviceId::from_public_key(&pk) != expected)
        {
            return self.abort(AbortReason::WrongDevice);
        }
        self.kind = Some(kind);
        self.peer = Some((pk, nonce));
        let (pk_i, pk_j) = self.pks_by_role();
        let key = derive_key(kind, &self.secret, &pk_i, &pk_j);
        self.words = Some(confirmation_words(&key, &pk_i, &pk_j));
        self.key = Some(key);
        self.deadline = now.saturating_add(PROOF_TIMEOUT_SECS);
        self.state = State::AwaitingProof;
        match self.role {
            // The inviter answers with its Hello and proves first.
            Role::Inviter => vec![self.hello(kind), self.own_proof()],
            // The joiner waits for the inviter's proof before revealing anything.
            Role::Joiner => Vec::new(),
        }
    }

    fn on_proof(&mut self, mac: &[u8; 32]) -> Vec<Message> {
        let peer_role = match self.role {
            Role::Inviter => Role::Joiner,
            Role::Joiner => Role::Inviter,
        };
        let Some((peer_pk, _)) = self.peer else {
            return self.abort(AbortReason::Unexpected);
        };
        let ok = self
            .proof_mac(peer_role, &peer_pk, &self.own_pk)
            .verify_slice(mac)
            .is_ok();
        if !ok {
            return self.abort(AbortReason::ProofFailed);
        }
        self.state = State::AwaitingConfirm {
            local: false,
            peer: false,
        };
        match self.role {
            Role::Inviter => Vec::new(),
            Role::Joiner => vec![self.own_proof()],
        }
    }

    fn own_proof(&self) -> Message {
        let mac = self.proof_mac(self.role, &self.own_pk, &self.peer_pk());
        Message::Proof {
            mac: mac.finalize().into_bytes().into(),
        }
    }

    fn peer_pk(&self) -> [u8; 32] {
        self.peer.map(|(pk, _)| pk).unwrap_or_default()
    }

    /// (inviter pk, joiner pk).
    fn pks_by_role(&self) -> ([u8; 32], [u8; 32]) {
        match self.role {
            Role::Inviter => (self.own_pk, self.peer_pk()),
            Role::Joiner => (self.peer_pk(), self.own_pk),
        }
    }

    /// (inviter nonce, joiner nonce).
    fn nonces_by_role(&self) -> ([u8; 32], [u8; 32]) {
        let peer_nonce = self.peer.map(|(_, n)| n).unwrap_or_default();
        match self.role {
            Role::Inviter => (self.own_nonce, peer_nonce),
            Role::Joiner => (peer_nonce, self.own_nonce),
        }
    }

    /// The HMAC state for the proof a `role` side with `prover_pk` sends to `peer_pk`.
    fn proof_mac(&self, role: Role, prover_pk: &[u8; 32], peer_pk: &[u8; 32]) -> HmacSha256 {
        let key: &[u8] = self.key.as_deref().map(|k| &k[..]).unwrap_or_default();
        let kind = self.kind.map(CodeKind::byte).unwrap_or_default();
        let (nonce_i, nonce_j) = self.nonces_by_role();
        let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts any key length");
        mac.update(b"wasl-pair");
        mac.update(&[kind, role.byte()]);
        mac.update(prover_pk);
        mac.update(peer_pk);
        mac.update(&nonce_i);
        mac.update(&nonce_j);
        mac
    }
}

fn derive_key(
    kind: CodeKind,
    secret: &[u8],
    pk_inviter: &[u8; 32],
    pk_joiner: &[u8; 32],
) -> Zeroizing<Vec<u8>> {
    match kind {
        CodeKind::Qr => Zeroizing::new(secret.to_vec()),
        CodeKind::Short => {
            let mut salt = Vec::with_capacity(15 + 64);
            salt.extend_from_slice(b"wasl-pair-short");
            salt.extend_from_slice(pk_inviter);
            salt.extend_from_slice(pk_joiner);
            let mut key = Zeroizing::new(vec![0u8; 32]);
            // The inviter holds the whole secret, the joiner only the short code's prefix of it.
            let short = &secret[..secret.len().min(SHORT_CODE_BYTES)];
            pbkdf2::pbkdf2_hmac::<Sha256>(short, &salt, SHORT_CODE_ROUNDS, &mut key);
            key
        }
    }
}

/// The four words both screens show.
fn confirmation_words(key: &[u8], pk_a: &[u8; 32], pk_b: &[u8; 32]) -> [&'static str; 4] {
    let (lo, hi) = if pk_a <= pk_b { (pk_a, pk_b) } else { (pk_b, pk_a) };
    let mut h = blake3::Hasher::new();
    h.update(b"wasl-words");
    h.update(lo);
    h.update(hi);
    h.update(key);
    let digest = h.finalize();
    let mut first = [0u8; 8];
    first.copy_from_slice(&digest.as_bytes()[..8]);
    let bits = u64::from_be_bytes(first);
    std::array::from_fn(|i| WORDS[((bits >> (64 - 11 * (i + 1))) & 0x7FF) as usize])
}

#[cfg(test)]
mod tests;
