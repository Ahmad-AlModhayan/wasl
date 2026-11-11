//! The op-log's rules (ADR-0002), apart from storage: states and outcomes, the moves between them,
//! actors and entity URIs. Nothing here touches SQL or another Wasl crate, so it can move into
//! `wasl-crdt` later as it is.

use std::fmt;

/// How long a decided changeset is kept: 90 days after `decided_at`, as `agent_changesets`
/// (`AGENT_HISTORY_MS` in `wasl-engine`).
pub const OPLOG_HISTORY_MS: i64 = 90 * 24 * 3600 * 1000;

/// Where a changeset is in its life (ADR-0002, States and moves).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OplogStatus {
    /// The session is still running; operations are appended.
    Open,
    /// Ended with changes to hold (Wasl's `held`, Qasd's waiting proposal).
    Staged,
    Committed,
    /// Staged and thrown away: nothing took effect.
    Discarded,
    Reverted,
    /// Reverted except for operations a revert skipped or could not take back.
    PartlyReverted,
}

impl OplogStatus {
    pub const ALL: [OplogStatus; 6] = [
        OplogStatus::Open,
        OplogStatus::Staged,
        OplogStatus::Committed,
        OplogStatus::Discarded,
        OplogStatus::Reverted,
        OplogStatus::PartlyReverted,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            OplogStatus::Open => "open",
            OplogStatus::Staged => "staged",
            OplogStatus::Committed => "committed",
            OplogStatus::Discarded => "discarded",
            OplogStatus::Reverted => "reverted",
            OplogStatus::PartlyReverted => "partly_reverted",
        }
    }

    pub fn parse(s: &str) -> Option<OplogStatus> {
        OplogStatus::ALL.into_iter().find(|v| v.as_str() == s)
    }

    /// A revert may run on it, and its operations' outcomes may be decided.
    pub fn revertible(self) -> bool {
        matches!(
            self,
            OplogStatus::Staged | OplogStatus::Committed | OplogStatus::PartlyReverted
        )
    }

    /// Pruned 90 days after it was decided; `open` and `staged` are never pruned.
    pub fn prunable(self) -> bool {
        !matches!(self, OplogStatus::Open | OplogStatus::Staged)
    }
}

impl fmt::Display for OplogStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What became of one operation (ADR-0002, Outcomes and append-only). Empty until decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpOutcome {
    /// It stands: set on commit, and on every inverse operation as it is appended.
    Applied,
    /// It never took effect.
    Discarded,
    /// A revert left it alone: the entity changed after the changeset (you win).
    SkippedChanged,
    /// A revert could not take it back.
    Failed,
    /// An inverse operation was appended for it.
    Reverted,
}

impl OpOutcome {
    pub const ALL: [OpOutcome; 5] = [
        OpOutcome::Applied,
        OpOutcome::Discarded,
        OpOutcome::SkippedChanged,
        OpOutcome::Failed,
        OpOutcome::Reverted,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            OpOutcome::Applied => "applied",
            OpOutcome::Discarded => "discarded",
            OpOutcome::SkippedChanged => "skipped_changed",
            OpOutcome::Failed => "failed",
            OpOutcome::Reverted => "reverted",
        }
    }

    pub fn parse(s: &str) -> Option<OpOutcome> {
        OpOutcome::ALL.into_iter().find(|v| v.as_str() == s)
    }
}

impl fmt::Display for OpOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A write the op-log turns down. Each names what was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OplogError {
    #[error("actor {0:?}: use agent:<name> (1 to 64 of a-z, 0-9, '.', '_', '-') or person:<device id>")]
    BadActor(String),
    #[error("device id {0:?}: expected WASL-XXXX-XXXX-XXXX-XXXX-XXXX-XXXX-XXXX as Wasl writes it")]
    BadDevice(String),
    #[error("entity URI {0:?}: expected file://<pair id>/<percent-encoded key> or another absolute URI")]
    BadEntityUri(String),
    #[error(
        "operation {op:?} on {entity:?}: a file is created (no before), updated (both) or deleted (no after)"
    )]
    BadOp { entity: String, op: String },
    #[error("changeset {0} is already in the op-log")]
    Exists(String),
    #[error("no changeset {0} in the op-log")]
    NoChangeset(String),
    #[error("changeset {id} has no operation {seq}")]
    NoOperation { id: String, seq: u32 },
    #[error("a changeset can't move from {from} to {to}")]
    BadMove { from: OplogStatus, to: OplogStatus },
    #[error("changeset {0} has no operations: drop it instead of ending it")]
    Empty(String),
    #[error("changeset {id} is {status}: only an open changeset takes operations, ends or is dropped")]
    NotOpen { id: String, status: OplogStatus },
    #[error("changeset {id} is {status}: only a staged changeset is accepted or discarded")]
    NotStaged { id: String, status: OplogStatus },
    #[error("changeset {0} has operations: only an empty open changeset is dropped")]
    NotEmpty(String),
    #[error("operation {seq}'s outcome can't move from {} to {to} while the changeset is {status}",
        .from.map_or("empty", OpOutcome::as_str))]
    BadOutcome {
        seq: u32,
        from: Option<OpOutcome>,
        to: OpOutcome,
        status: OplogStatus,
    },
    #[error("operation {0} is an inverse operation: it is neither reverted nor decided again")]
    InverseOp(u32),
    #[error("operation {0} is marked reverted only by appending its inverse (a revert)")]
    NeedsInverse(u32),
    #[error("imported operation {seq} reverts {reverts}, which is not an earlier operation")]
    BadReverts { seq: u32, reverts: u32 },
}

/// Whether a changeset may move from `from` to `to` (ADR-0002, States and moves). Creating a
/// changeset (new → `open`) and dropping an empty one (`open` → deleted) are not moves between
/// states and are checked where they happen.
pub fn move_allowed(from: OplogStatus, to: OplogStatus) -> bool {
    use OplogStatus::*;
    matches!(
        (from, to),
        (Open, Staged | Committed)
            | (Staged, Committed | Discarded)
            | (Staged | Committed, Reverted | PartlyReverted)
            | (PartlyReverted, Reverted | PartlyReverted)
    )
}

pub fn check_move(from: OplogStatus, to: OplogStatus) -> Result<(), OplogError> {
    if move_allowed(from, to) {
        Ok(())
    } else {
        Err(OplogError::BadMove { from, to })
    }
}

/// Whether an operation's outcome may move from `from` to `to` while its changeset is `status`
/// (ADR-0002, Outcomes and append-only). Outcomes are decided only while a revert may run
/// (`staged`, `committed`, `partly_reverted`); the moves a commit or discard makes themselves
/// (empty to `applied` or `discarded`) happen inside that move.
pub fn outcome_allowed(status: OplogStatus, from: Option<OpOutcome>, to: OpOutcome) -> bool {
    use OpOutcome::*;
    if !status.revertible() {
        return false;
    }
    match from {
        None => true,
        Some(Applied) => matches!(to, SkippedChanged | Failed | Reverted),
        // Decided again by a resumed or later revert.
        Some(SkippedChanged | Failed) => matches!(to, SkippedChanged | Failed | Reverted),
        Some(Discarded | Reverted) => false,
    }
}

pub fn check_outcome(
    seq: u32,
    status: OplogStatus,
    from: Option<OpOutcome>,
    to: OpOutcome,
) -> Result<(), OplogError> {
    if outcome_allowed(status, from, to) {
        Ok(())
    } else {
        Err(OplogError::BadOutcome {
            seq,
            from,
            to,
            status,
        })
    }
}

/// Where a revert leaves a changeset: `reverted` when every operation that took effect (each
/// original not `discarded`) is taken back, else `partly_reverted`. `outcomes` are the originals'
/// outcomes, not the inverse operations'.
pub fn revert_result(outcomes: impl IntoIterator<Item = Option<OpOutcome>>) -> OplogStatus {
    let all_back = outcomes
        .into_iter()
        .all(|o| matches!(o, Some(OpOutcome::Reverted | OpOutcome::Discarded)));
    if all_back {
        OplogStatus::Reverted
    } else {
        OplogStatus::PartlyReverted
    }
}

/// The operation that takes `op` back: `create` ↔ `delete`; any other operation is its own
/// inverse (with `prev` and `next` swapped).
pub fn inverse_op(op: &str) -> &str {
    match op {
        "create" => "delete",
        "delete" => "create",
        other => other,
    }
}

/// Checks an actor (ADR-0002, Actors): `agent:<name>` with `<name>` in `[a-z0-9._-]{1,64}`, or
/// `person:<device id>`.
pub fn check_actor(actor: &str) -> Result<(), OplogError> {
    let ok = if let Some(name) = actor.strip_prefix("agent:") {
        (1..=64).contains(&name.len())
            && name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-'))
    } else if let Some(device) = actor.strip_prefix("person:") {
        is_device_id(device)
    } else {
        false
    };
    if ok {
        Ok(())
    } else {
        Err(OplogError::BadActor(actor.to_owned()))
    }
}

/// Wasl's base32 alphabet: `0-9 A-Z` without `I`, `L`, `O`, `U`.
const ID_ALPHABET: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// A device id as Wasl writes it (`DeviceId`'s `Display`): `WASL-` and seven groups of four
/// symbols of Wasl's base32 alphabet. The shape only: the check symbols are `wasl-net`'s to verify.
fn is_device_id(s: &str) -> bool {
    let Some(rest) = s.strip_prefix("WASL") else {
        return false;
    };
    let b = rest.as_bytes();
    b.len() == 7 * 5
        && b.chunks(5)
            .all(|g| g[0] == b'-' && g[1..].iter().all(|c| ID_ALPHABET.contains(c)))
}

pub fn check_device(device: &str) -> Result<(), OplogError> {
    if is_device_id(device) {
        Ok(())
    } else {
        Err(OplogError::BadDevice(device.to_owned()))
    }
}

fn unreserved(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~')
}

/// Percent-encodes every byte that is not unreserved (RFC 3986 §2.3), so `/`, spaces, `#`, `?`,
/// `%` and non-ASCII names all round-trip.
fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if unreserved(b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = std::str::from_utf8(b.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// A path segment or authority as `file_uri` writes it: unreserved characters, sub-delims, `:`,
/// `@` and `%XX` (RFC 3986 `pchar`), not `.` or `..`.
fn is_segment(s: &str) -> bool {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' => {
                if !(b.get(i + 1).is_some_and(u8::is_ascii_hexdigit)
                    && b.get(i + 2).is_some_and(u8::is_ascii_hexdigit))
                {
                    return false;
                }
                i += 3;
            }
            c if unreserved(c) || b"!$&'()*+,;=:@".contains(&c) => i += 1,
            _ => return false,
        }
    }
    !s.is_empty() && s != "." && s != ".."
}

/// `file://<pair id or watch id>/<key>` with the id and each segment of the key percent-encoded.
/// An empty key gives the pair root `file://<id>/`; for a folder scope add the trailing `/`
/// ([`folder_uri`]).
pub fn file_uri(id: &str, key: &str) -> String {
    let path: Vec<String> = if key.is_empty() {
        Vec::new()
    } else {
        key.split('/').map(encode).collect()
    };
    format!("file://{}/{}", encode(id), path.join("/"))
}

/// A folder scope: [`file_uri`] ending in `/`.
pub fn folder_uri(id: &str, key: &str) -> String {
    let uri = file_uri(id, key);
    if uri.ends_with('/') { uri } else { uri + "/" }
}

/// The id and key of a `file://` URI, decoded; `None` for another scheme or a URI
/// [`check_entity_uri`] refuses. A folder's key keeps no trailing `/`.
pub fn file_uri_parts(uri: &str) -> Option<(String, String)> {
    let rest = uri.strip_prefix("file://")?;
    let (id, path) = rest.split_once('/')?;
    let mut segments: Vec<&str> = path.split('/').collect();
    if segments.last() == Some(&"") {
        segments.pop();
    }
    if !is_segment(id) || !segments.iter().all(|s| is_segment(s)) {
        return None;
    }
    let key: Option<Vec<String>> = segments.iter().map(|s| decode(s)).collect();
    Some((decode(id)?, key?.join("/")))
}

/// Checks an entity URI (ADR-0002, Entity URIs). `file://` is checked in full; any other scheme
/// only for being an absolute URI (a scheme, `:`, and no spaces or control characters), and is
/// then kept byte for byte.
pub fn check_entity_uri(uri: &str) -> Result<(), OplogError> {
    let bad = || OplogError::BadEntityUri(uri.to_owned());
    let (scheme, rest) = uri.split_once(':').ok_or_else(bad)?;
    let scheme_ok = scheme.bytes().next().is_some_and(|b| b.is_ascii_alphabetic())
        && scheme
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.'));
    if !scheme_ok || rest.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(bad());
    }
    if scheme.eq_ignore_ascii_case("file") {
        // One spelling, so the same file is always the same URI.
        if scheme != "file" || file_uri_parts(uri).is_none() {
            return Err(bad());
        }
    }
    Ok(())
}

/// Checks an operation's name against its entity: a `file://` entity is created (no `prev`),
/// updated (both) or deleted (no `next`); another scheme names its own operations.
pub fn check_op(entity: &str, op: &str, has_prev: bool, has_next: bool) -> Result<(), OplogError> {
    let ok = if entity.starts_with("file://") {
        matches!(
            (op, has_prev, has_next),
            ("create", false, true) | ("update", true, true) | ("delete", true, false)
        )
    } else {
        !op.is_empty() && !op.chars().any(|c| c.is_whitespace() || c.is_control())
    };
    if ok {
        Ok(())
    } else {
        Err(OplogError::BadOp {
            entity: entity.to_owned(),
            op: op.to_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_uris_round_trip_any_key() {
        for key in ["a.txt", "docs/a b#c?d%e.md", "تقارير/ملف.txt", "x/y/z"] {
            let uri = file_uri("p-1", key);
            check_entity_uri(&uri).unwrap();
            assert_eq!(Some(("p-1".to_owned(), key.to_owned())), file_uri_parts(&uri));
        }
        assert_eq!("file://p/", file_uri("p", ""));
        assert_eq!("file://p/a/b/", folder_uri("p", "a/b"));
        assert_eq!("file://p/", folder_uri("p", ""));
    }

    #[test]
    fn every_move_outside_the_table_is_refused() {
        use OplogStatus::*;
        let allowed = [
            (Open, Staged),
            (Open, Committed),
            (Staged, Committed),
            (Staged, Discarded),
            (Staged, Reverted),
            (Staged, PartlyReverted),
            (Committed, Reverted),
            (Committed, PartlyReverted),
            (PartlyReverted, Reverted),
            (PartlyReverted, PartlyReverted),
        ];
        for from in OplogStatus::ALL {
            for to in OplogStatus::ALL {
                assert_eq!(
                    allowed.contains(&(from, to)),
                    move_allowed(from, to),
                    "{from} → {to}"
                );
            }
        }
    }
}
