//! Reading the op-log (ADR-0002, Read format; SRS FR-OL8): what `wasl oplog list/show --json` and
//! the IPC answer, for the products that read it (Raqeeb, Qasd, Miqyas). Read-only: nothing here
//! stages, commits or reverts.

use crate::{Engine, EngineError, Result};
use chrono::{DateTime, SecondsFormat};
use wasl_store::{OplogChangeset, OplogFilter, OplogKey, OplogOp};

pub use wasl_proto::oplog::{OPLOG_FORMAT_VERSION, OplogEntry, OplogOpEntry, OplogPage};

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

fn oplog_entry(cs: OplogChangeset) -> Result<OplogEntry> {
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

fn oplog_op_entry(k: &OplogKey, op: OplogOp) -> Result<OplogOpEntry> {
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

impl Engine {
    /// The changesets `f` matches, newest first and at most `f.limit`, each with its `op_count`.
    pub fn oplog_list(&self, f: &OplogFilter) -> Result<OplogPage> {
        let listed = self.store.lock().oplog_list(f)?;
        let changesets = listed
            .into_iter()
            .map(|l| {
                Ok(OplogEntry {
                    op_count: Some(l.op_count),
                    ..oplog_entry(l.changeset)?
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
                .map(|op| oplog_op_entry(&k, op))
                .collect::<Result<_>>()?;
            OplogPage {
                version: OPLOG_FORMAT_VERSION,
                changesets: vec![OplogEntry {
                    operations: Some(operations),
                    ..oplog_entry(cs)?
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
