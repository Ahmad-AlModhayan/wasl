//! `wasl oplog`: the op-log read-only (docs/adr/0002-one-op-log.md, SRS FR-OL8). Nothing here
//! stages, commits or reverts; accepting stays in the Wasl window.

use crate::OplogCmd;
use crate::cmd::{Fail, NOT_FOUND, OK, R, USAGE, connect, find_pair, print_json};
use crate::out::{self, Tone, paint};
use wasl_cli::{i18n, t};
use wasl_engine::{OplogEntry, OplogPage};
use wasl_store::{OplogFilter, OplogStatus};

pub async fn run(cmd: OplogCmd) -> R<u8> {
    match cmd {
        OplogCmd::List {
            actor,
            status,
            since,
            pair,
            limit,
            json,
        } => list(actor, status, since, pair, limit, json).await,
        OplogCmd::Show { id, json } => show(&id, json).await,
    }
}

/// `<device>:<id>`, or a bare `<id>` for this device. Neither part holds a `:`.
fn reference(r: &str) -> (Option<&str>, &str) {
    match r.split_once(':') {
        Some((device, id)) => (Some(device), id),
        None => (None, r),
    }
}

fn status(s: &str) -> R<OplogStatus> {
    OplogStatus::parse(s).ok_or_else(|| {
        let all: Vec<&str> = OplogStatus::ALL.iter().map(|s| s.as_str()).collect();
        Fail::new(USAGE, t!(OplogBadStatus, s, all.join(", ")))
    })
}

/// A time in milliseconds: an RFC 3339 time, a date (the start of that local day), or `30m`, `24h`,
/// `7d` before `now`. A count below zero would be in the future, so it is refused.
fn since(s: &str, now: i64) -> R<i64> {
    use chrono::{DateTime, Local, NaiveDate, TimeZone};
    let bad = || Fail::new(USAGE, t!(OplogBadSince, s));
    if let Ok(t) = DateTime::parse_from_rfc3339(s) {
        return Ok(t.timestamp_millis());
    }
    if let Ok(d) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return day_start(d, |t| {
            Local
                .from_local_datetime(&t)
                .earliest()
                .map(|t| t.timestamp_millis())
        })
        .ok_or_else(bad);
    }
    let unit = match s.chars().last() {
        Some('m') => 60_000,
        Some('h') => 3_600_000,
        Some('d') => 86_400_000,
        _ => return Err(bad()),
    };
    let n: i64 = s[..s.len() - 1].parse().map_err(|_| bad())?;
    if n < 0 {
        return Err(bad());
    }
    n.checked_mul(unit)
        .and_then(|back| now.checked_sub(back))
        .ok_or_else(bad)
}

/// The first time of day `d` that `at` (local time to milliseconds) knows: midnight, or where the
/// clocks skip it for summer time (America/Santiago, 00:00 to 01:00), the first quarter hour after.
fn day_start(d: chrono::NaiveDate, at: impl Fn(chrono::NaiveDateTime) -> Option<i64>) -> Option<i64> {
    let midnight = d.and_hms_opt(0, 0, 0)?;
    (0..96).find_map(|q| at(midnight + chrono::Duration::minutes(15 * q)))
}

fn ms(rfc3339: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(rfc3339)
        .ok()
        .map(|t| t.timestamp_millis())
}

fn when(rfc3339: &str) -> String {
    ms(rfc3339).map(out::when).unwrap_or_else(|| "-".into())
}

fn status_label(s: OplogStatus) -> String {
    match s {
        OplogStatus::Open => t!(OplogStOpen),
        OplogStatus::Staged => paint(&t!(OplogStStaged), Tone::Busy),
        OplogStatus::Committed => t!(OplogStCommitted),
        OplogStatus::Discarded => t!(OplogStDiscarded),
        OplogStatus::Reverted => t!(OplogStReverted),
        OplogStatus::PartlyReverted => t!(OplogStPartly),
    }
}

/// The scope without `file://<pair id>`, as `wasl agent list` shows a path; other schemes whole.
fn scope_label(scope: &str, pair: &str) -> String {
    match wasl_store::file_uri_parts(scope) {
        Some((id, key)) if id == pair => format!("/{key}"),
        _ => scope.to_owned(),
    }
}

async fn list(
    actor: Option<String>,
    status_arg: Option<String>,
    since_arg: Option<String>,
    pair: Option<String>,
    limit: u32,
    json: bool,
) -> R<u8> {
    // The daemon answers at most this many, so the note below compares with what it can return.
    let limit = limit.min(wasl_ipc::OPLOG_LIST_MAX);
    let c = connect().await?;
    let pairs = c.pairs().await?;
    let pair = match pair {
        // A deleted pair's changesets stay until pruned: an unknown reference is taken as its id.
        Some(p) => match find_pair(&pairs, &p) {
            Ok(found) => Some(found.id.clone()),
            Err(f) if f.code == NOT_FOUND => {
                eprintln!("{}", t!(OplogPairAsId, i18n::clean(&p)));
                Some(p)
            }
            Err(f) => return Err(f),
        },
        None => None,
    };
    let filter = OplogFilter {
        actor,
        status: status_arg.as_deref().map(status).transpose()?,
        since: since_arg
            .as_deref()
            .map(|s| since(s, out::now_ms()))
            .transpose()?,
        pair,
        limit: Some(limit),
    };
    let page = c.oplog_list(&filter).await?;
    if json {
        return print_json(&page);
    }
    if page.changesets.is_empty() {
        println!("{}", t!(OplogNone));
        return Ok(OK);
    }
    // `show <bare id>` means this device, so only this device's changesets list a bare id.
    let me = c
        .devices()
        .await?
        .devices
        .into_iter()
        .find(|i| i.this_device)
        .map(|i| i.id);
    print!("{}", out::table(&list_rows(&page, &pairs, me.as_deref())));
    // As many as asked for: there may be older ones.
    if page.changesets.len() == limit as usize {
        eprintln!("{}", t!(OplogMore, limit));
    }
    Ok(OK)
}

/// The rows of `wasl oplog list`, header first. A changeset from this device (`me`) has a bare id,
/// one from another device `<device>:<id>`, as `wasl oplog show` takes them.
fn list_rows(page: &OplogPage, pairs: &[wasl_store::Pair], me: Option<&str>) -> Vec<Vec<String>> {
    let mut rows = vec![vec![
        t!(ColId),
        t!(ColActor),
        t!(ColPair),
        t!(ColScope),
        t!(ColStarted),
        t!(ColState),
        t!(ColChanges),
    ]];
    for cs in &page.changesets {
        let (pair_id, _) = wasl_store::file_uri_parts(&cs.scope).unwrap_or_default();
        let pair = pairs
            .iter()
            .find(|p| p.id == pair_id)
            .map(|p| i18n::clean(&p.name))
            .unwrap_or_else(|| pair_id.clone());
        rows.push(vec![
            if me == Some(cs.origin_device.as_str()) {
                cs.id.clone()
            } else {
                format!("{}:{}", cs.origin_device, cs.id)
            },
            cs.actor.clone(),
            pair,
            scope_label(&cs.scope, &pair_id),
            when(&cs.opened_at),
            status_label(cs.status),
            cs.op_count.map(|n| n.to_string()).unwrap_or_default(),
        ]);
    }
    rows
}

async fn show(r: &str, json: bool) -> R<u8> {
    let c = connect().await?;
    let (device, id) = reference(r);
    let page = c.oplog_show(device, id).await?;
    if json {
        return print_json(&page);
    }
    let Some(cs) = page.changesets.first() else {
        return Err(Fail::new(NOT_FOUND, t!(AgentNoSuch, r)));
    };
    print!("{}", show_text(cs));
    Ok(OK)
}

/// What `wasl oplog show` prints: a header line, the label, who decided, then the operations.
fn show_text(cs: &OplogEntry) -> String {
    let mut s = format!(
        "{}:{}  {}  {}  {}\n",
        cs.origin_device,
        cs.id,
        cs.actor,
        i18n::clean(&cs.scope),
        status_label(cs.status)
    );
    if let Some(label) = cs.label.as_deref().map(i18n::clean).filter(|l| !l.is_empty()) {
        s += &format!("  {label}\n");
    }
    if let (Some(actor), Some(by), Some(at)) = (&cs.decided_actor, &cs.decided_by, &cs.decided_at) {
        s += &format!("  {}\n", t!(OplogDecided, actor, by, when(at)));
    }
    let mut rows = vec![vec![t!(ColSeq), t!(ColChange), t!(ColEntity), t!(ColResult)]];
    for op in cs.operations.iter().flatten() {
        let seq = match op.reverts {
            Some(n) => format!("{} ← {n}", op.seq),
            None => op.seq.to_string(),
        };
        rows.push(vec![
            seq,
            op.op.clone(),
            i18n::clean(&op.entity),
            op.outcome.map(|o| o.as_str().to_owned()).unwrap_or_default(),
        ]);
    }
    s += &out::table(&rows);
    s
}

#[cfg(test)]
mod tests {
    use super::{day_start, list_rows, since};

    const NOW: i64 = 1_790_000_000_000;

    #[test]
    fn since_takes_minutes_hours_and_days_before_now() {
        assert_eq!(Ok(NOW - 30 * 60_000), since("30m", NOW).map_err(|f| f.code));
        assert_eq!(Ok(NOW - 24 * 3_600_000), since("24h", NOW).map_err(|f| f.code));
        assert_eq!(Ok(NOW - 7 * 86_400_000), since("7d", NOW).map_err(|f| f.code));
    }

    #[test]
    fn since_takes_a_date_as_local_midnight_and_a_time_as_given() {
        use chrono::{Local, TimeZone};
        let midnight = Local
            .with_ymd_and_hms(2026, 11, 1, 0, 0, 0)
            .earliest()
            .unwrap()
            .timestamp_millis();
        assert_eq!(Ok(midnight), since("2026-11-01", NOW).map_err(|f| f.code));
        assert_eq!(
            Ok(1_790_812_800_000),
            since("2026-10-01T00:00:00Z", NOW).map_err(|f| f.code)
        );
    }

    #[test]
    fn a_day_whose_midnight_is_skipped_starts_at_its_first_time() {
        use chrono::{NaiveDate, Timelike};
        let d = NaiveDate::from_ymd_opt(2026, 9, 6).unwrap();
        // Like America/Santiago that day: the clocks go from 00:00 to 01:00, at UTC-3.
        let santiago =
            |t: chrono::NaiveDateTime| (t.hour() >= 1).then(|| (t.and_utc().timestamp() + 3 * 3600) * 1000);
        let one_am = d.and_hms_opt(1, 0, 0).unwrap().and_utc().timestamp_millis();
        assert_eq!(Some(one_am + 3 * 3_600_000), day_start(d, santiago));
        let utc = |t: chrono::NaiveDateTime| Some(t.and_utc().timestamp_millis());
        let midnight = d.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp_millis();
        assert_eq!(
            Some(midnight),
            day_start(d, utc),
            "a day with its midnight starts there"
        );
        assert_eq!(None, day_start(d, |_| None), "a zone with no time that day: none");
    }

    #[test]
    fn since_refuses_a_count_below_zero_or_too_large() {
        for s in [
            "-7d",
            "-1m",
            "-9223372036854775807d",
            "9223372036854775807d",
            "7w",
            "d",
        ] {
            assert_eq!(
                Err(crate::cmd::USAGE),
                since(s, NOW).map(|_| ()).map_err(|f| f.code),
                "{s}"
            );
        }
    }

    #[test]
    fn only_this_devices_changesets_list_a_bare_id() {
        let entry = |device: &str| {
            serde_json::json!({
                "origin_device": device, "id": "1790000000000-1", "actor": "agent:claude-code",
                "label": null, "scope": "file://p/", "status": "staged",
                "opened_at": "2026-09-21T14:13:20.000Z", "closed_at": null, "decided_at": null,
                "decided_by": null, "decided_actor": null, "imported_at": null
            })
        };
        let page: wasl_engine::OplogPage = serde_json::from_value(serde_json::json!({
            "version": 1, "changesets": [entry("WASL-ME"), entry("WASL-PHONE")]
        }))
        .unwrap();
        let rows = list_rows(&page, &[], Some("WASL-ME"));
        assert_eq!("1790000000000-1", rows[1][0]);
        assert_eq!("WASL-PHONE:1790000000000-1", rows[2][0]);
        let rows = list_rows(&page, &[], None);
        assert_eq!(
            "WASL-ME:1790000000000-1", rows[1][0],
            "this device unknown: every id whole"
        );
    }
}
