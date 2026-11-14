//! Op-log schema rules and JSON read-format DTOs (ADR-0002).

pub mod rules;

pub use rules::{
    OPLOG_HISTORY_MS, OpOutcome, OplogError, OplogStatus, check_actor, check_device, check_entity_uri,
    check_move, check_op, check_outcome, file_uri, file_uri_parts, folder_uri, inverse_op, move_allowed,
    outcome_allowed, revert_result,
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A changeset's name: ids are unique only on the device that made them.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct OplogKey {
    pub origin_device: String,
    pub id: String,
}

impl OplogKey {
    pub fn new(origin_device: impl Into<String>, id: impl Into<String>) -> OplogKey {
        OplogKey {
            origin_device: origin_device.into(),
            id: id.into(),
        }
    }
}

/// A session beginning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewChangeset {
    pub key: OplogKey,
    /// `agent:<name>` or `person:<device id>`.
    pub actor: String,
    pub label: Option<String>,
    /// Entity URI of the folder (ends in `/`) or other container.
    pub scope: String,
    pub opened_at: i64,
}

/// One change to one entity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewOp {
    pub entity_uri: String,
    /// For `file://`: `create`, `update` or `delete`.
    pub op: String,
    /// The state before and after; `None` for a created or deleted entity's missing side.
    pub prev: Option<Value>,
    pub next: Option<Value>,
    pub at: i64,
}

/// Who decided an accept, discard or revert.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    /// The client: `cli`, `desktop`, `unknown`.
    pub by: String,
    /// The agent when an agent asked, else `person:<device id>`.
    pub actor: String,
    pub at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OplogChangeset {
    pub origin_device: String,
    pub id: String,
    pub actor: String,
    pub label: Option<String>,
    pub scope: String,
    pub status: OplogStatus,
    pub opened_at: i64,
    pub closed_at: Option<i64>,
    pub decided_at: Option<i64>,
    pub decided_by: Option<String>,
    pub decided_actor: Option<String>,
    pub imported_at: Option<i64>,
}

impl OplogChangeset {
    pub fn key(&self) -> OplogKey {
        OplogKey::new(&self.origin_device, &self.id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OplogOp {
    pub seq: u32,
    pub entity_uri: String,
    pub op: String,
    pub prev: Option<Value>,
    pub next: Option<Value>,
    /// On an inverse operation: the `seq` it takes back.
    pub reverts: Option<u32>,
    pub outcome: Option<OpOutcome>,
    pub at: i64,
}

/// Which changesets [`Store::oplog_list`] returns; each field left `None` matches all.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OplogFilter {
    pub actor: Option<String>,
    pub status: Option<OplogStatus>,
    /// Opened at or after this, milliseconds.
    pub since: Option<i64>,
    /// A pair (or watch) id: changesets whose scope is in it (`file://<id>/…`).
    pub pair: Option<String>,
    /// At most this many, the newest.
    pub limit: Option<u32>,
}

/// A changeset in a list, with how many operations it has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OplogListed {
    pub changeset: OplogChangeset,
    pub op_count: u32,
}

/// An operation copied from v10 as it is, numbered in order from 1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportOp {
    pub op: NewOp,
    pub reverts: Option<u32>,
    pub outcome: Option<OpOutcome>,
}

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

/// Converts milliseconds since the Unix epoch to an RFC 3339 UTC timestamp with millisecond
/// precision (`YYYY-MM-DDTHH:MM:SS.mmmZ`), returning `None` if the year falls outside `0000..=9999`.
pub fn format_rfc3339_utc(ms: i64) -> Option<String> {
    const MS_PER_DAY: i64 = 86_400_000;
    let days = ms.div_euclid(MS_PER_DAY);
    let day_ms = ms.rem_euclid(MS_PER_DAY) as u64;

    // Howard Hinnant's civil-from-days algorithm shifted from 1970-01-01 to 0000-03-01.
    let z = days.checked_add(719_468)?;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let year = y + i64::from(m <= 2);
    if !(0..=9999).contains(&year) {
        return None;
    }

    let total_secs = day_ms / 1000;
    let millis = day_ms % 1000;
    let hour = total_secs / 3600;
    let minute = (total_secs % 3600) / 60;
    let second = total_secs % 60;
    Some(format!(
        "{year:04}-{m:02}-{d:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z"
    ))
}

#[cfg(test)]
mod tests {
    use super::format_rfc3339_utc;

    #[test]
    fn rfc3339_utc_formats_epoch_and_rejects_extremes() {
        assert_eq!(Some("1970-01-01T00:00:00.000Z".into()), format_rfc3339_utc(0));
        assert_eq!(Some("1969-12-31T23:59:59.999Z".into()), format_rfc3339_utc(-1));
        assert_eq!(
            Some("2026-09-21T14:13:20.000Z".into()),
            format_rfc3339_utc(1_790_000_000_000)
        );
        assert_eq!(None, format_rfc3339_utc(i64::MAX));
        assert_eq!(None, format_rfc3339_utc(i64::MIN));
    }
}
