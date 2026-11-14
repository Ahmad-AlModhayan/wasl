//! The group (docs/DEVICE_SYNC.md §3): the signed list of one person's devices.
//!
//! The group is a set of signed **operations**: `create` (the first device), `add` and `remove`.
//! Each operation names the operations its signer had seen (`parents`), so together they form a
//! graph, and every device that holds the same operations computes the same trusted set, whatever
//! order they arrived in. Operations are written as canonical JSON (object keys sorted by byte
//! order, no whitespace); IDs as `WASL-…`, keys, hashes and signatures as lower-case hex. The
//! signature is Ed25519 over the canonical JSON without `signature`; an operation's ID is the
//! BLAKE3 of its full canonical JSON.
//!
//! Who counts, given the operations ([`GroupOps::state`]):
//! - An operation is signed by a member: a device added (or created) by a counted operation in
//!   its past, and not removed by one there.
//! - A removal is permanent: a removed ID is never a member again (that device re-pairs with a
//!   new key).
//! - An addition by a device made concurrently with that device's removal (neither in the
//!   other's past), or after it, does not count. So a device cannot use the moments around its
//!   removal, or old parents it forges later, to bring anyone in; and devices it brought in
//!   before its removal stay.
//! - A removal concurrent with its signer's own removal still counts. Two devices removing each
//!   other at once both leave (fail closed). A removed device can forge such a removal of a device
//!   that existed before its own removal; that device then re-pairs. This is no more than what it
//!   could do while it was a member, and it can never stay or bring anyone in.
//!
//! That last rule was kept on purpose when operations began to travel (docs/SECURITY.md S2-2).
//! Voiding such a removal would lose an honest one: a device offline for a week removes an old
//! tablet, and is itself removed elsewhere meanwhile; the tablet would stay trusted. Any exception
//! that depends on which removals count (only the remover may be removed back, only the first
//! one, only devices it added) lets a removed device set removals against each other until the
//! rounds below stop settling, and then its own removal can be the one left out. So the bound is
//! where the operations come from instead: a device asks only members for operations and reads
//! only members' files, so a removed device reaches only devices that have not heard of its
//! removal yet, a window the shared group files close within a poll; and each signer's
//! operations are taken in at a bounded rate ([`SignerThrottle`]).
//!
//! Which operations count is settled by rounds, each judging every operation by the previous
//! round's verdicts, so an operation withdrawn only because of one that turns out not to count
//! comes back; rounds that do not settle leave out whatever failed in either of the last two.
//!
//! Operations that do not count are not stored when they arrive ([`GroupOps::insert`]), and each
//! signer may store a bounded number, so a removed device cannot fill the log.

use std::collections::{BTreeMap, HashMap};
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::identity::{DeviceId, DeviceKey, parse_public_key, verify_signature};

/// Most members of a group.
pub const MAX_DEVICES: usize = 256;
/// Longest device name, in bytes.
pub const MAX_NAME_BYTES: usize = 128;
/// Largest operation accepted, in bytes.
pub const MAX_OP_BYTES: usize = 16 << 10;
/// Most parents one operation names.
pub const MAX_PARENTS: usize = 16;
/// Most operations a group keeps.
pub const MAX_OPS: usize = 1024;
/// Most operations one signer may have in the group.
pub const MAX_OPS_PER_SIGNER: usize = 256;
/// Most operations kept while waiting for their parents.
pub const MAX_PENDING: usize = 256;
/// Most of those from one signer.
pub const MAX_PENDING_PER_SIGNER: usize = 16;
/// Most waiting operations from signers not known yet.
pub const MAX_PENDING_UNKNOWN: usize = 32;
/// Rounds [`GroupOps`] spends settling which operations count.
const EVALUATION_ROUNDS: usize = 16;
/// Most new operations from one signer taken in per minute ([`SignerThrottle`]).
pub const OPS_PER_SIGNER_PER_MINUTE: usize = 64;
/// Most signers a [`SignerThrottle`] keeps track of.
pub const THROTTLE_SIGNERS: usize = 4 * MAX_DEVICES;

/// The ID of an operation: BLAKE3 of its canonical JSON.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct OpId(#[serde(with = "hex")] pub [u8; 32]);

impl fmt::Debug for OpId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for b in &self.0[..6] {
            write!(f, "{b:02x}")?;
        }
        Ok(())
    }
}

/// Why an operation was refused.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum GroupError {
    #[error("group operation is larger than {MAX_OP_BYTES} bytes")]
    TooLarge,
    #[error("group operation is not valid: {0}")]
    Malformed(String),
    #[error("group operation is signed by {0}, which is not a member there")]
    UntrustedSigner(DeviceId),
    #[error("group operation signature does not verify")]
    BadSignature,
    #[error("{0} is not in the group")]
    NotMember(DeviceId),
    #[error("{0} is already in the group")]
    AlreadyMember(DeviceId),
    #[error("{0} was removed from the group and cannot return with the same key")]
    Revoked(DeviceId),
    #[error("the group already holds as many operations as it may")]
    Full,
    #[error("this is not the group's first operation")]
    WrongGenesis,
}

/// One device in the group.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GroupDevice {
    pub id: DeviceId,
    pub name: String,
    #[serde(with = "hex")]
    pub pk: [u8; 32],
    /// Unix seconds.
    pub added_at: u64,
    pub added_by: DeviceId,
}

/// What an operation does.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Change {
    /// The first device; the group's only operation without parents.
    Create(GroupDevice),
    Add(GroupDevice),
    Remove(DeviceId),
}

/// A signed group operation as stored and exchanged. Untrusted until [`GroupOps::insert`] keeps
/// it.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GroupOp {
    pub change: Change,
    /// The operations the signer had seen, sorted, without repeats.
    pub parents: Vec<OpId>,
    pub signed_by: DeviceId,
    #[serde(with = "hex")]
    pub signature: [u8; 64],
}

impl fmt::Debug for GroupOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GroupOp")
            .field("change", &self.change)
            .field("parents", &self.parents)
            .field("signed_by", &self.signed_by)
            .finish_non_exhaustive()
    }
}

impl GroupOp {
    /// Parses an operation (size-limited, unknown fields refused). Not verified.
    pub fn from_json(bytes: &[u8]) -> Result<Self, GroupError> {
        if bytes.len() > MAX_OP_BYTES {
            return Err(GroupError::TooLarge);
        }
        serde_json::from_slice(bytes).map_err(|e| GroupError::Malformed(e.to_string()))
    }

    /// The operation as canonical JSON, signature included.
    pub fn to_json(&self) -> Vec<u8> {
        let value = serde_json::to_value(self).expect("an operation is always JSON");
        canonical_json(&value).into_bytes()
    }

    /// The bytes the signature covers: canonical JSON without `signature`.
    pub fn signed_bytes(&self) -> Vec<u8> {
        let mut value = serde_json::to_value(self).expect("an operation is always JSON");
        if let Value::Object(map) = &mut value {
            map.remove("signature");
        }
        canonical_json(&value).into_bytes()
    }

    pub fn id(&self) -> OpId {
        OpId(*blake3::hash(&self.to_json()).as_bytes())
    }

    fn signed(change: Change, parents: Vec<OpId>, signer: &DeviceKey) -> Self {
        let mut op = Self {
            change,
            parents,
            signed_by: signer.id(),
            signature: [0; 64],
        };
        op.signature = signer.sign(&op.signed_bytes());
        op
    }

    /// The device this operation brings in, if any.
    fn device(&self) -> Option<&GroupDevice> {
        match &self.change {
            Change::Create(d) | Change::Add(d) => Some(d),
            Change::Remove(_) => None,
        }
    }

    /// Structure only: bounded sizes, sorted parents, a valid key whose ID matches, a device
    /// added by its signer, a create without parents and an add or remove with some.
    fn validate(&self) -> Result<(), GroupError> {
        let bad = |why: &str| Err(GroupError::Malformed(why.to_string()));
        if self.parents.len() > MAX_PARENTS {
            return bad("too many parents");
        }
        if self.parents.windows(2).any(|w| w[0] >= w[1]) {
            return bad("parents are not sorted and unique");
        }
        match &self.change {
            Change::Create(_) if !self.parents.is_empty() => return bad("create has parents"),
            Change::Add(_) | Change::Remove(_) if self.parents.is_empty() => return bad("no parents"),
            _ => {}
        }
        if let Some(d) = self.device() {
            if d.name.len() > MAX_NAME_BYTES || d.name.trim().is_empty() {
                return bad("device name is empty or too long");
            }
            if parse_public_key(&d.pk).is_err() || DeviceId::from_public_key(&d.pk) != d.id {
                return bad("device ID does not match its key");
            }
            if d.added_by != self.signed_by {
                return bad("added_by is not the signer");
            }
            if matches!(self.change, Change::Create(_)) && d.id != self.signed_by {
                return bad("create is not signed by the device it creates");
            }
        }
        Ok(())
    }
}

/// The trusted set: who is in the group now.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Group {
    devices: Vec<GroupDevice>,
    removed: Vec<DeviceId>,
}

impl Group {
    pub fn devices(&self) -> &[GroupDevice] {
        &self.devices
    }

    /// Devices removed from the group; their keys are never trusted again.
    pub fn removed(&self) -> &[DeviceId] {
        &self.removed
    }

    /// The member with this ID.
    pub fn device(&self, id: &DeviceId) -> Option<&GroupDevice> {
        self.devices.iter().find(|d| d.id == *id)
    }

    /// Whether this public key belongs to a member (the transport's pin check).
    pub fn trusts_key(&self, pk: &[u8; 32]) -> bool {
        self.devices.iter().any(|d| d.pk == *pk)
    }
}

/// Every group operation a device holds, and the trusted set they add up to. Persist it with
/// [`GroupOps::to_json`] and load it with [`GroupOps::from_json`], which checks it all again.
#[derive(Clone, Debug)]
pub struct GroupOps {
    genesis: OpId,
    ops: BTreeMap<OpId, GroupOp>,
    pending: Vec<GroupOp>,
    state: Group,
}

/// What [`GroupOps`] computes from the graph.
struct Evaluation {
    /// Operations in a fixed topological order (parents first, then by ID).
    order: Vec<OpId>,
    /// For each position, the positions of its ancestors.
    past: Vec<Vec<bool>>,
    counts: Vec<bool>,
}

impl GroupOps {
    /// A new group holding only this device, and the operation that creates it.
    pub fn create(key: &DeviceKey, name: &str, now: u64) -> Result<(Self, GroupOp), GroupError> {
        let me = GroupDevice {
            id: key.id(),
            name: name.to_string(),
            pk: key.public(),
            added_at: now,
            added_by: key.id(),
        };
        let op = GroupOp::signed(Change::Create(me), Vec::new(), key);
        op.validate()?;
        let mut ops = BTreeMap::new();
        ops.insert(op.id(), op.clone());
        let mut group = Self {
            genesis: op.id(),
            ops,
            pending: Vec::new(),
            state: Group::default(),
        };
        group.recompute();
        Ok((group, op))
    }

    /// Adopts a group from a peer this device has just paired with (the only key it trusts so
    /// far): `ops` must hold exactly one create, and the peer must be a member of the result.
    pub fn bootstrap(ops: &[GroupOp], peer_pk: &[u8; 32]) -> Result<Self, GroupError> {
        let mut creates = ops.iter().filter(|o| matches!(o.change, Change::Create(_)));
        let (Some(genesis), None) = (creates.next(), creates.next()) else {
            return Err(GroupError::WrongGenesis);
        };
        let group = Self::load(ops, genesis)?;
        if !group.state.trusts_key(peer_pk) {
            return Err(GroupError::UntrustedSigner(DeviceId::from_public_key(peer_pk)));
        }
        Ok(group)
    }

    /// Builds the group from `genesis` and every operation in `ops` that counts.
    fn load(ops: &[GroupOp], genesis: &GroupOp) -> Result<Self, GroupError> {
        genesis.validate()?;
        let d = genesis.device().ok_or(GroupError::WrongGenesis)?;
        if !verify_signature(&d.pk, &genesis.signed_bytes(), &genesis.signature) {
            return Err(GroupError::BadSignature);
        }
        let mut group = Self {
            genesis: genesis.id(),
            ops: BTreeMap::from([(genesis.id(), genesis.clone())]),
            pending: Vec::new(),
            state: Group::default(),
        };
        // Keep every well-formed operation signed by a device added somewhere in its past, then
        // decide once over all of them which count. Operations that once counted and no longer do
        // (a removal arrived later) load like any other. An operation whose parents are missing
        // or whose signature does not check means the set was damaged: refused.
        let mut rest: Vec<&GroupOp> = ops
            .iter()
            .filter(|o| o.id() != group.genesis && !matches!(o.change, Change::Create(_)))
            .collect();
        loop {
            let before = rest.len();
            let mut i = 0;
            while i < rest.len() {
                let op = rest[i];
                if op.parents.iter().all(|p| group.ops.contains_key(p)) {
                    op.validate()?;
                    if !group.signature_checks(op) {
                        return Err(GroupError::BadSignature);
                    }
                    if group.ops.len() >= MAX_OPS
                        || group.ops.values().filter(|o| o.signed_by == op.signed_by).count()
                            >= MAX_OPS_PER_SIGNER
                    {
                        return Err(GroupError::Full);
                    }
                    group.ops.insert(op.id(), op.clone());
                    rest.swap_remove(i);
                } else {
                    i += 1;
                }
            }
            if rest.len() == before {
                break;
            }
        }
        if !rest.is_empty() {
            return Err(GroupError::Malformed(
                "operations whose parents are missing".into(),
            ));
        }
        group.recompute();
        Ok(group)
    }

    /// Whether `op` is signed by the key of a device added (or created) somewhere in its past.
    fn signature_checks(&self, op: &GroupOp) -> bool {
        self.key_in_past(&op.parents, &op.signed_by)
            .is_some_and(|pk| verify_signature(&pk, &op.signed_bytes(), &op.signature))
    }

    /// The key of device `id` as added by an operation at or before `from`.
    fn key_in_past(&self, from: &[OpId], id: &DeviceId) -> Option<[u8; 32]> {
        let mut stack: Vec<OpId> = from.to_vec();
        let mut seen: std::collections::HashSet<OpId> = std::collections::HashSet::new();
        while let Some(o) = stack.pop() {
            if !seen.insert(o) {
                continue;
            }
            let Some(op) = self.ops.get(&o) else { continue };
            if let Some(d) = op.device()
                && d.id == *id
            {
                return Some(d.pk);
            }
            stack.extend(op.parents.iter().copied());
        }
        None
    }

    /// Who is in the group.
    pub fn state(&self) -> &Group {
        &self.state
    }

    /// Every operation kept, parents first (to send to a peer or write to Drive).
    pub fn ops(&self) -> Vec<&GroupOp> {
        let eval = self.evaluate();
        eval.order.iter().map(|id| &self.ops[id]).collect()
    }

    /// Whether this operation is already kept.
    pub fn contains(&self, id: &OpId) -> bool {
        self.ops.contains_key(id)
    }

    /// Whether this operation is kept, or waiting here for its parents.
    pub fn holds(&self, id: &OpId) -> bool {
        self.ops.contains_key(id) || self.pending.iter().any(|p| p.id() == *id)
    }

    /// Whose [`SignerThrottle`] share taking `op` in would use, judged before
    /// [`GroupOps::insert`] (which still makes every check). The signature is checked here so
    /// that whoever can hand this device operations (a group file in Drive can be written by
    /// anyone holding the account's token, a removed device included) cannot use up a member's
    /// share by claiming its name.
    pub fn screen(&self, op: &GroupOp) -> Screened {
        if op.validate().is_err() || matches!(op.change, Change::Create(_)) {
            return Screened::Refused;
        }
        if self.holds(&op.id()) {
            return Screened::Held;
        }
        match self.known_key(&op.signed_by) {
            Some(pk) if verify_signature(&pk, &op.signed_bytes(), &op.signature) => {
                Screened::Signed(op.signed_by)
            }
            Some(_) => Screened::Refused,
            None => Screened::Unknown,
        }
    }

    /// The key of device `id`, as any kept operation brought it in. A device ID is derived from
    /// its key ([`GroupOp::validate`]), so there is at most one; a removed device keeps it.
    fn known_key(&self, id: &DeviceId) -> Option<[u8; 32]> {
        self.ops
            .values()
            .find_map(|o| o.device().filter(|d| d.id == *id).map(|d| d.pk))
    }

    /// The operations nothing kept builds on yet: the parents of the next operation.
    pub fn heads(&self) -> Vec<OpId> {
        let mut has_child: HashMap<OpId, bool> = self.ops.keys().map(|k| (*k, false)).collect();
        for op in self.ops.values() {
            for p in &op.parents {
                has_child.insert(*p, true);
            }
        }
        let mut heads: Vec<OpId> = has_child
            .into_iter()
            .filter(|(_, c)| !c)
            .map(|(k, _)| k)
            .collect();
        heads.sort();
        heads
    }

    /// Parents for a new operation: every head when they fit; otherwise heads whose past adds
    /// each of `need` (the signer, and the device being removed) first, then the rest in order,
    /// so the operation always carries the history that makes it count.
    fn parents_for(&self, need: &[DeviceId]) -> Vec<OpId> {
        let heads = self.heads();
        if heads.len() <= MAX_PARENTS {
            return heads;
        }
        let mut chosen: Vec<OpId> = Vec::new();
        for d in need {
            if !chosen.iter().any(|h| self.key_in_past(&[*h], d).is_some())
                && let Some(h) = heads.iter().find(|h| self.key_in_past(&[**h], d).is_some())
            {
                chosen.push(*h);
            }
        }
        for h in &heads {
            if chosen.len() >= MAX_PARENTS {
                break;
            }
            if !chosen.contains(h) {
                chosen.push(*h);
            }
        }
        chosen.sort();
        chosen
    }

    /// The operation adding `pk`, signed by `signer` (a member). Keep it with
    /// [`GroupOps::insert`] and send it to the other devices.
    pub fn add_device(
        &self,
        pk: [u8; 32],
        name: &str,
        now: u64,
        signer: &DeviceKey,
    ) -> Result<GroupOp, GroupError> {
        self.check_signer(signer)?;
        let id = DeviceId::from_public_key(&pk);
        if self.state.device(&id).is_some() {
            return Err(GroupError::AlreadyMember(id));
        }
        if self.state.removed.contains(&id) {
            return Err(GroupError::Revoked(id));
        }
        if self.state.devices.len() >= MAX_DEVICES {
            return Err(GroupError::Full);
        }
        let device = GroupDevice {
            id,
            name: name.to_string(),
            pk,
            added_at: now,
            added_by: signer.id(),
        };
        let op = GroupOp::signed(Change::Add(device), self.parents_for(&[signer.id()]), signer);
        op.validate()?;
        Ok(op)
    }

    /// The operation removing `id`, signed by `signer` (a member; it may remove itself).
    pub fn remove_device(&self, id: DeviceId, signer: &DeviceKey) -> Result<GroupOp, GroupError> {
        self.check_signer(signer)?;
        if self.state.device(&id).is_none() {
            return Err(GroupError::NotMember(id));
        }
        Ok(GroupOp::signed(
            Change::Remove(id),
            self.parents_for(&[signer.id(), id]),
            signer,
        ))
    }

    fn check_signer(&self, signer: &DeviceKey) -> Result<(), GroupError> {
        if self.state.trusts_key(&signer.public()) {
            Ok(())
        } else {
            Err(GroupError::NotMember(signer.id()))
        }
    }

    /// Keeps `op` if it counts; true if it is new. An operation whose parents are not all here
    /// yet waits (bounded) and is retried as they arrive. Refused: bad structure or signature, a
    /// signer that is not a member in the operation's past, an operation that would not count,
    /// and a signer over its share.
    pub fn insert(&mut self, op: GroupOp) -> Result<bool, GroupError> {
        op.validate()?;
        let id = op.id();
        if self.ops.contains_key(&id) || self.pending.iter().any(|p| p.id() == id) {
            return Ok(false);
        }
        if matches!(op.change, Change::Create(_)) {
            return Err(GroupError::WrongGenesis);
        }
        if op.parents.iter().any(|p| !self.ops.contains_key(p)) {
            // A signer whose key is known here is checked now and waits in the main place, at most
            // MAX_PENDING_PER_SIGNER at a time. A signer not known yet (added by an operation still
            // on its way) waits in a small place of its own, where the oldest gives way; it can
            // never push out a known member's operation.
            let known = match self.state.device(&op.signed_by).map(|d| d.pk) {
                Some(pk) if verify_signature(&pk, &op.signed_bytes(), &op.signature) => true,
                Some(_) => return Err(GroupError::BadSignature),
                None => false,
            };
            if !known {
                let unknown: Vec<usize> = (0..self.pending.len())
                    .filter(|i| self.state.device(&self.pending[*i].signed_by).is_none())
                    .collect();
                if unknown.len() >= MAX_PENDING_UNKNOWN {
                    self.pending.remove(unknown[0]);
                }
                self.pending.push(op);
                return Ok(true);
            }
            if self
                .pending
                .iter()
                .filter(|p| p.signed_by == op.signed_by)
                .count()
                >= MAX_PENDING_PER_SIGNER
            {
                return Err(GroupError::Full);
            }
            if self.pending.len() >= MAX_PENDING {
                // Make room from whoever holds the most places (never the newcomer's own share).
                let mut held: HashMap<DeviceId, usize> = HashMap::new();
                for p in &self.pending {
                    *held.entry(p.signed_by).or_default() += 1;
                }
                let hog = held
                    .into_iter()
                    .max_by_key(|(id, n)| (*n, *id.as_bytes()))
                    .map(|(id, _)| id)
                    .filter(|id| *id != op.signed_by)
                    .ok_or(GroupError::Full)?;
                let oldest = self
                    .pending
                    .iter()
                    .position(|p| p.signed_by == hog)
                    .expect("held");
                self.pending.remove(oldest);
            }
            self.pending.push(op);
            return Ok(true);
        }
        self.admit(op)?;
        // Waiting operations whose parents are now here.
        loop {
            let ready = self
                .pending
                .iter()
                .position(|p| p.parents.iter().all(|q| self.ops.contains_key(q)));
            let Some(i) = ready else { break };
            let p = self.pending.swap_remove(i);
            let _ = self.admit(p);
        }
        Ok(true)
    }

    fn admit(&mut self, op: GroupOp) -> Result<(), GroupError> {
        if self.ops.len() >= MAX_OPS {
            return Err(GroupError::Full);
        }
        if self.ops.values().filter(|o| o.signed_by == op.signed_by).count() >= MAX_OPS_PER_SIGNER {
            return Err(GroupError::Full);
        }
        // The signer must be a member as its parents saw it, by the operations that count now.
        let eval = self.evaluate();
        let pos: HashMap<OpId, usize> = eval.order.iter().enumerate().map(|(i, id)| (*id, i)).collect();
        let mut past = vec![false; eval.order.len()];
        for p in &op.parents {
            let i = pos[p];
            past[i] = true;
            for (j, a) in eval.past[i].iter().enumerate() {
                past[j] |= *a;
            }
        }
        let signer = self
            .member_in(&eval, &past, &op.signed_by)
            .ok_or(GroupError::UntrustedSigner(op.signed_by))?;
        if !verify_signature(&signer.pk, &op.signed_bytes(), &op.signature) {
            return Err(GroupError::BadSignature);
        }
        let (id, signed_by) = (op.id(), op.signed_by);
        self.ops.insert(id, op);
        if !self.evaluate_counts(id) {
            self.ops.remove(&id);
            return Err(GroupError::UntrustedSigner(signed_by));
        }
        self.recompute();
        Ok(())
    }

    fn evaluate_counts(&self, id: OpId) -> bool {
        let eval = self.evaluate();
        eval.order
            .iter()
            .position(|o| *o == id)
            .is_some_and(|i| eval.counts[i])
    }

    /// The member `id` as the operations marked in `past` (that count) see it.
    fn member_in(&self, eval: &Evaluation, past: &[bool], id: &DeviceId) -> Option<GroupDevice> {
        let mut found = None;
        for (i, op_id) in eval.order.iter().enumerate() {
            if !past[i] || !eval.counts[i] {
                continue;
            }
            match &self.ops[op_id].change {
                Change::Create(d) | Change::Add(d) if d.id == *id => found = Some(d.clone()),
                Change::Remove(r) if r == id => return None,
                _ => {}
            }
        }
        found
    }

    fn recompute(&mut self) {
        let eval = self.evaluate();
        let mut devices: Vec<GroupDevice> = Vec::new();
        let mut removed: Vec<DeviceId> = Vec::new();
        for (i, id) in eval.order.iter().enumerate() {
            if !eval.counts[i] {
                continue;
            }
            match &self.ops[id].change {
                Change::Create(d) | Change::Add(d) => {
                    if !devices.iter().any(|x| x.id == d.id) {
                        devices.push(d.clone());
                    }
                }
                Change::Remove(r) => {
                    if !removed.contains(r) {
                        removed.push(*r);
                    }
                }
            }
        }
        devices.retain(|d| !removed.contains(&d.id));
        self.state = Group { devices, removed };
    }

    /// Orders the graph and decides which operations count (see the module docs). Starts with
    /// every operation counting and only ever withdraws, so it ends, and in the same place on
    /// every device that holds the same operations.
    fn evaluate(&self) -> Evaluation {
        // Kahn's algorithm with a sorted ready set: a fixed order.
        let mut children: HashMap<OpId, Vec<OpId>> = HashMap::new();
        let mut waiting: HashMap<OpId, usize> = HashMap::new();
        for (id, op) in &self.ops {
            waiting.insert(*id, op.parents.len());
            for p in &op.parents {
                children.entry(*p).or_default().push(*id);
            }
        }
        let mut ready: std::collections::BTreeSet<OpId> = waiting
            .iter()
            .filter(|(_, n)| **n == 0)
            .map(|(id, _)| *id)
            .collect();
        let mut order = Vec::with_capacity(self.ops.len());
        while let Some(id) = ready.pop_first() {
            order.push(id);
            for c in children.get(&id).into_iter().flatten() {
                let n = waiting.get_mut(c).expect("child is kept");
                *n -= 1;
                if *n == 0 {
                    ready.insert(*c);
                }
            }
        }
        let pos: HashMap<OpId, usize> = order.iter().enumerate().map(|(i, id)| (*id, i)).collect();
        let n = order.len();
        let mut past = vec![vec![false; n]; n];
        for i in 0..n {
            for p in &self.ops[&order[i]].parents {
                let j = pos[p];
                past[i][j] = true;
                let (before, after) = past.split_at_mut(i);
                for (k, a) in before[j].iter().enumerate() {
                    after[0][k] |= *a;
                }
            }
        }

        // Only the genesis may have no parents; anything else must descend from it.
        let g = pos[&self.genesis];
        let rooted: Vec<bool> = (0..n).map(|i| i == g || past[i][g]).collect();
        // Every operation is judged against the previous round's verdicts, starting from "all
        // count", until nothing changes: an operation withdrawn only because of one that later
        // turns out not to count comes back. If the rounds do not settle, whatever failed in
        // either of the last two stays out (fail closed). The same operations give the same
        // verdicts on every device.
        let mut counts = rooted.clone();
        let mut previous = counts.clone();
        for _ in 0..EVALUATION_ROUNDS {
            let next: Vec<bool> = (0..n)
                .map(|i| {
                    i == g
                        || (rooted[i]
                            && Self::counts_at(&self.ops, &order, &past, &counts, i, &self.ops[&order[i]]))
                })
                .collect();
            if next == counts {
                previous = counts.clone();
                break;
            }
            previous = std::mem::replace(&mut counts, next);
        }
        let counts = counts.iter().zip(&previous).map(|(a, b)| *a && *b).collect();
        Evaluation { order, past, counts }
    }

    fn counts_at(
        ops: &BTreeMap<OpId, GroupOp>,
        order: &[OpId],
        past: &[Vec<bool>],
        counts: &[bool],
        i: usize,
        op: &GroupOp,
    ) -> bool {
        let signer = op.signed_by;
        // Members and removals as this operation's past sees them.
        let mut signer_added = false;
        let mut target_added = false;
        let mut removed_in_past: Vec<DeviceId> = Vec::new();
        for (j, id) in order.iter().enumerate() {
            if !past[i][j] || !counts[j] {
                continue;
            }
            match &ops[id].change {
                Change::Create(d) | Change::Add(d) => {
                    signer_added |= d.id == signer;
                    if let Change::Remove(t) = &op.change {
                        target_added |= d.id == *t;
                    }
                }
                Change::Remove(r) => removed_in_past.push(*r),
            }
        }
        if !signer_added || removed_in_past.contains(&signer) {
            return false;
        }
        match &op.change {
            Change::Create(_) => false,
            Change::Remove(_) => target_added,
            Change::Add(d) => {
                if removed_in_past.contains(&d.id) {
                    return false;
                }
                // Void if the signer's removal (counted) is anywhere but in this operation's
                // future: before it was handled above; concurrent is here.
                !order.iter().enumerate().any(|(j, id)| {
                    counts[j]
                        && j != i
                        && !past[j][i]
                        && matches!(&ops[id].change, Change::Remove(r) if *r == signer)
                })
            }
        }
    }

    /// The operations as JSON, to persist.
    pub fn to_json(&self) -> Vec<u8> {
        let ops: Vec<Value> = self
            .ops()
            .into_iter()
            .map(|o| serde_json::to_value(o).expect("an operation is always JSON"))
            .collect();
        serde_json::to_vec(&ops).expect("JSON")
    }

    /// Loads persisted operations, checking every one again. `genesis` is the create this device
    /// trusted when it made or joined the group.
    pub fn from_json(bytes: &[u8], genesis: OpId) -> Result<Self, GroupError> {
        let ops: Vec<GroupOp> =
            serde_json::from_slice(bytes).map_err(|e| GroupError::Malformed(e.to_string()))?;
        let create = ops
            .iter()
            .find(|o| o.id() == genesis)
            .ok_or(GroupError::WrongGenesis)?;
        if !matches!(create.change, Change::Create(_)) {
            return Err(GroupError::WrongGenesis);
        }
        Self::load(&ops, create)
    }

    /// The create this group grew from.
    pub fn genesis(&self) -> OpId {
        self.genesis
    }

    /// The operations kept here that a device whose group has `heads` lacks, parents first: every
    /// operation outside the past of the heads known here (heads unknown here are the other
    /// device's news, fetched the other way). How members spread additions and removals to each
    /// other (security review 2026-10).
    pub fn missing_for(&self, heads: &[OpId]) -> Vec<GroupOp> {
        let mut seen: std::collections::HashSet<OpId> = std::collections::HashSet::new();
        let mut stack: Vec<OpId> = heads
            .iter()
            .copied()
            .filter(|h| self.ops.contains_key(h))
            .collect();
        while let Some(o) = stack.pop() {
            if !seen.insert(o) {
                continue;
            }
            if let Some(op) = self.ops.get(&o) {
                stack.extend(op.parents.iter().copied());
            }
        }
        self.ops()
            .into_iter()
            .filter(|o| !seen.contains(&o.id()))
            .cloned()
            .collect()
    }
}

/// What [`GroupOps::screen`] found an operation to be, deciding whose throttle share it uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screened {
    /// Kept already, or waiting for its parents: costs nothing when it comes again.
    Held,
    /// Malformed, a create, or a signature that does not check against its signer's key here:
    /// dropped, charged to no one.
    Refused,
    /// Signed with its signer's key: charged to that signer.
    Signed(DeviceId),
    /// From a signer whose key is not known here yet (added by an operation still on its way):
    /// charged to one share all such signers have together, so a made-up signer ID neither uses
    /// a member's share nor opens one of its own.
    Unknown,
}

/// How many new operations from each signer a device takes in per minute (S2-2). Every insert
/// evaluates the whole graph, so one signer (a member gone rogue, or a removed device reaching a
/// device that has not heard yet) must not keep a device busy, or push its operations in faster
/// than the news of its removal travels. An operation over the share is skipped, not refused for
/// good: the member that sent it still has it and sends it again at the next exchange. Honest
/// devices sign a handful of operations a year, so catching up after a long time away takes at
/// most a few minutes.
#[derive(Debug, Default)]
pub struct SignerThrottle {
    /// Per signer, when (unix seconds) each operation in the last minute was taken.
    taken: HashMap<DeviceId, std::collections::VecDeque<u64>>,
    /// The same, for every signer not known yet together ([`Screened::Unknown`]).
    unknown: std::collections::VecDeque<u64>,
}

impl SignerThrottle {
    /// Whether one more operation from `signer` may be taken in at `now` (unix seconds); counts it
    /// if so.
    pub fn allow(&mut self, signer: DeviceId, now: u64) -> bool {
        let fresh = |t: &u64| now.saturating_sub(*t) < 60;
        self.taken.retain(|_, times| {
            times.retain(fresh);
            !times.is_empty()
        });
        if !self.taken.contains_key(&signer) && self.taken.len() >= THROTTLE_SIGNERS {
            // Bounded: forget whoever was quietest longest (made-up signer IDs cannot grow it).
            let quietest = self
                .taken
                .iter()
                .min_by_key(|(id, times)| (times.back().copied(), *id.as_bytes()))
                .map(|(id, _)| *id);
            if let Some(id) = quietest {
                self.taken.remove(&id);
            }
        }
        let times = self.taken.entry(signer).or_default();
        if times.len() >= OPS_PER_SIGNER_PER_MINUTE {
            return false;
        }
        times.push_back(now);
        true
    }

    /// Whether one more operation from a signer not known yet may be taken in at `now`; counts
    /// it if so. All such signers share one minute's share.
    pub fn allow_unknown(&mut self, now: u64) -> bool {
        self.unknown.retain(|t| now.saturating_sub(*t) < 60);
        if self.unknown.len() >= OPS_PER_SIGNER_PER_MINUTE {
            return false;
        }
        self.unknown.push_back(now);
        true
    }
}

/// Canonical JSON: object keys sorted by byte order, no whitespace.
pub fn canonical_json(value: &Value) -> String {
    let mut out = String::new();
    write_canonical(value, &mut out);
    out
}

fn write_canonical(value: &Value, out: &mut String) {
    match value {
        Value::Object(map) => {
            let mut entries: Vec<(&String, &Value)> = map.iter().collect();
            entries.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
            out.push('{');
            for (i, (k, v)) in entries.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String(k.clone()).to_string());
                out.push(':');
                write_canonical(v, out);
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (i, v) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_canonical(v, out);
            }
            out.push(']');
        }
        scalar => out.push_str(&scalar.to_string()),
    }
}

/// Fixed-length byte arrays as lower-case hex.
pub(crate) mod hex {
    use serde::{Deserialize, Deserializer, Serializer, de::Error};

    pub fn serialize<S: Serializer, const N: usize>(bytes: &[u8; N], s: S) -> Result<S::Ok, S::Error> {
        let mut text = String::with_capacity(N * 2);
        for b in bytes {
            text.push(char::from_digit(u32::from(b >> 4), 16).unwrap_or('0'));
            text.push(char::from_digit(u32::from(b & 15), 16).unwrap_or('0'));
        }
        s.serialize_str(&text)
    }

    pub fn deserialize<'de, D: Deserializer<'de>, const N: usize>(d: D) -> Result<[u8; N], D::Error> {
        let text = <std::borrow::Cow<'de, str>>::deserialize(d)?;
        let digit = |c: u8| match c {
            b'0'..=b'9' => Some(c - b'0'),
            b'a'..=b'f' => Some(c - b'a' + 10),
            _ => None,
        };
        let raw = text.as_bytes();
        if raw.len() != N * 2 {
            return Err(D::Error::custom("wrong hex length"));
        }
        let mut out = [0u8; N];
        for (i, [hi, lo]) in raw.as_chunks::<2>().0.iter().enumerate() {
            match (digit(*hi), digit(*lo)) {
                (Some(hi), Some(lo)) => out[i] = (hi << 4) | lo,
                _ => return Err(D::Error::custom("not lower-case hex")),
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests;
