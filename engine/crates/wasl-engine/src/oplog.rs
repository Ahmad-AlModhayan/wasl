//! Reading the op-log (ADR-0002, Read format; SRS FR-OL8): what `wasl oplog list/show --json` and
//! the IPC answer, for the products that read it (Raqeeb, Qasd, Miqyas). Read-only: nothing here
//! stages, commits or reverts.

use crate::{Engine, EngineError, Result};
use chrono::{DateTime, SecondsFormat};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use wasl_store::{OpOutcome, OplogChangeset, OplogFilter, OplogKey, OplogOp, OplogStatus};

/// The read format's version. Adding a field keeps it; removing or renaming one raises it.
pub const OPLOG_FORMAT_VERSION: u32 = 1;

/// One answer: `{"version": 1, "changesets": [...]}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OplogPage {
    pub version: u32,
    pub changesets: Vec<OplogEntry>,
}

/// A changeset by its column names, times as RFC 3339 UTC, and the origin device's name. A list
/// adds `op_count`, a show adds `operations`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OplogEntry {
    pub origin_device: String,
    /// The origin device's name, from the device list or this device's own; `None` when unknown.
    #[serde(default)]
    pub origin_device_name: Option<String>,
    pub id: String,
    pub actor: String,
    pub label: Option<String>,
    pub scope: String,
    pub status: OplogStatus,
    pub opened_at: String,
    pub closed_at: Option<String>,
    pub decided_at: Option<String>,
    pub decided_by: Option<String>,
    pub decided_actor: Option<String>,
    pub imported_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub op_count: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operations: Option<Vec<OplogOpEntry>>,
}

/// An operation by its column names: `entity_uri` as `entity`, states as JSON objects.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OplogOpEntry {
    #[serde(default)]
    pub origin_device: String,
    #[serde(default)]
    pub changeset_id: String,
    pub seq: u32,
    pub entity: String,
    pub op: String,
    pub prev: Option<Value>,
    pub next: Option<Value>,
    pub reverts: Option<u32>,
    pub outcome: Option<OpOutcome>,
    pub at: String,
}

/// Milliseconds since the Unix epoch as RFC 3339 UTC, to the millisecond. A time past what a date
/// can hold (about 262,000 years either way) is a damaged row: refused, never printed as `""`.
pub fn rfc3339(ms: i64) -> Result<String> {
    DateTime::from_timestamp_millis(ms)
        .map(|t| t.to_rfc3339_opts(SecondsFormat::Millis, true))
        .ok_or_else(|| EngineError::Integrity(format!("op-log time out of range: {ms} ms")))
}

/// [`rfc3339`] for a time of changeset `origin_device:id`, which a refusal names so the damaged
/// row can be found.
fn rfc3339_in(ms: i64, origin_device: &str, id: &str) -> Result<String> {
    rfc3339(ms).map_err(|_| {
        EngineError::Integrity(format!(
            "op-log time out of range in changeset {origin_device}:{id}: {ms} ms"
        ))
    })
}

impl OplogEntry {
    pub fn new(cs: OplogChangeset) -> Result<OplogEntry> {
        let at = |ms: i64| rfc3339_in(ms, &cs.origin_device, &cs.id);
        let opened_at = at(cs.opened_at)?;
        let closed_at = cs.closed_at.map(at).transpose()?;
        let decided_at = cs.decided_at.map(at).transpose()?;
        let imported_at = cs.imported_at.map(at).transpose()?;
        Ok(OplogEntry {
            origin_device: cs.origin_device,
            origin_device_name: None,
            id: cs.id,
            actor: cs.actor,
            label: cs.label,
            scope: cs.scope,
            status: cs.status,
            opened_at,
            closed_at,
            decided_at,
            decided_by: cs.decided_by,
            decided_actor: cs.decided_actor,
            imported_at,
            op_count: None,
            operations: None,
        })
    }
}

impl OplogOpEntry {
    pub fn new(k: &OplogKey, op: OplogOp) -> Result<OplogOpEntry> {
        Ok(OplogOpEntry {
            origin_device: k.origin_device.clone(),
            changeset_id: k.id.clone(),
            seq: op.seq,
            entity: op.entity_uri,
            op: op.op,
            prev: op.prev,
            next: op.next,
            reverts: op.reverts,
            outcome: op.outcome,
            at: rfc3339_in(op.at, &k.origin_device, &k.id)?,
        })
    }
}

impl OplogPage {
    /// Sets `origin_device_name` on each changeset `name_of` knows the origin of.
    pub fn name_devices(&mut self, name_of: impl Fn(&str) -> Option<String>) {
        for cs in &mut self.changesets {
            if let Some(name) = name_of(&cs.origin_device) {
                cs.origin_device_name = Some(name);
            }
        }
    }
}

impl Engine {
    /// The changesets `f` matches, newest first and at most `f.limit`, each with its `op_count`.
    pub fn oplog_list(&self, f: &OplogFilter) -> Result<OplogPage> {
        let listed = self.store.lock().oplog_list(f)?;
        let changesets = listed
            .into_iter()
            .map(|l| {
                Ok(OplogEntry {
                    op_count: Some(l.op_count),
                    ..OplogEntry::new(l.changeset)?
                })
            })
            .collect::<Result<_>>()?;
        self.named(OplogPage {
            version: OPLOG_FORMAT_VERSION,
            changesets,
        })
    }

    /// `page` with this device's own name on its changesets; the device list names the others.
    fn named(&self, mut page: OplogPage) -> Result<OplogPage> {
        let Some(me) = self.store.lock().oplog_device().map(str::to_owned) else {
            return Ok(page);
        };
        let name = self.device_name()?;
        page.name_devices(|d| (d == me).then(|| name.clone()));
        Ok(page)
    }

    /// One changeset with its `operations`; `origin_device` `None` is this device.
    pub fn oplog_show(&self, origin_device: Option<&str>, id: &str) -> Result<OplogPage> {
        let page = {
            let s = self.store.lock();
            let not_found = || EngineError::NotFound(format!("changeset {id}"));
            let device = match origin_device {
                Some(d) => d.to_owned(),
                None => s.oplog_device().ok_or_else(not_found)?.to_owned(),
            };
            let k = OplogKey::new(device, id);
            let cs = s.oplog_changeset(&k)?.ok_or_else(not_found)?;
            let operations = s
                .oplog_ops(&k)?
                .into_iter()
                .map(|op| OplogOpEntry::new(&k, op))
                .collect::<Result<_>>()?;
            OplogPage {
                version: OPLOG_FORMAT_VERSION,
                changesets: vec![OplogEntry {
                    operations: Some(operations),
                    ..OplogEntry::new(cs)?
                }],
            }
        };
        self.named(page)
    }
}

#[cfg(test)]
mod tests {
    use super::{rfc3339, rfc3339_in};

    #[test]
    fn a_time_past_what_a_date_holds_is_refused_not_printed_empty() {
        assert_eq!("2026-09-21T14:13:20.000Z", rfc3339(1_790_000_000_000).unwrap());
        for ms in [i64::MAX, i64::MIN] {
            let e = rfc3339(ms).unwrap_err();
            assert!(e.to_string().contains(&ms.to_string()), "{e}");
            let e = rfc3339_in(ms, "dev-a", "cs-1").unwrap_err().to_string();
            assert!(e.contains("dev-a:cs-1") && e.contains(&ms.to_string()), "{e}");
        }
    }
}
