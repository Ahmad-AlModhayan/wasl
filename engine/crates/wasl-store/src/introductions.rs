//! Introductions (docs/INTRODUCTIONS.md, 2026-10): which devices a shared pair is served to, what
//! this device knows of each member, and the folders other devices offer this one.
//!
//! Device IDs are kept as text (`WASL-XXXX-…`), share IDs as hex. The store does not know the
//! group: callers ignore rows naming a device that is no longer a member.

use crate::{Result, Store, StoreError, to_i64, to_u64};
use rusqlite::{OptionalExtension, params};
use std::collections::{HashMap, HashSet, VecDeque};

/// Where an offer this device received stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OfferState {
    /// Not answered yet: shown to the person.
    New,
    /// Hidden on this device; the sender sees "not accepted yet" and can withdraw it.
    Declined,
    /// A local pair was made for it; kept even after the sender stops offering it.
    Accepted,
}

impl OfferState {
    pub fn as_str(self) -> &'static str {
        match self {
            OfferState::New => "new",
            OfferState::Declined => "declined",
            OfferState::Accepted => "accepted",
        }
    }

    pub fn parse(s: &str) -> Option<OfferState> {
        [OfferState::New, OfferState::Declined, OfferState::Accepted]
            .into_iter()
            .find(|o| o.as_str() == s)
    }
}

/// An offer as a sender makes it, before this device keeps it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewOffer {
    /// The share ID (hex).
    pub share: String,
    pub name: String,
    pub files: u64,
    pub bytes: u64,
}

/// A folder another device offered this one (§3.3), one row per (share, sender).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceivedOffer {
    /// The share ID (hex).
    pub share: String,
    /// The device that offered it (`WASL-…`).
    pub from_device: String,
    pub name: String,
    pub files: u64,
    pub bytes: u64,
    /// Unix ms, by this device's clock.
    pub first_seen: i64,
    pub state: OfferState,
}

impl Store {
    /// Whether a pair's share is served to every member known here (shares made before
    /// introductions, and shares made without a device list); `None` when it is not shared.
    pub fn share_all_devices(&self, pair_id: &str) -> Result<Option<bool>> {
        Ok(self
            .conn
            .query_row(
                "SELECT all_devices FROM pair_shares WHERE pair_id = ?1",
                [pair_id],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// The devices a pair is shared with by name, sorted.
    pub fn share_devices(&self, pair_id: &str) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT device_id FROM pair_share_devices WHERE pair_id = ?1 ORDER BY device_id",
        )?;
        let rows = stmt.query_map([pair_id], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Whether `device` is on the pair's list.
    pub fn shared_with(&self, pair_id: &str, device: &str) -> Result<bool> {
        Ok(self
            .conn
            .query_row(
                "SELECT 1 FROM pair_share_devices WHERE pair_id = ?1 AND device_id = ?2",
                params![pair_id, device],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    /// Sets who a shared pair is served to: every known member (`all`) and the listed devices,
    /// replacing the list. The pair must be shared already.
    pub fn set_share_scope(&mut self, pair_id: &str, all: bool, devices: &[String]) -> Result<()> {
        let tx = self.conn.transaction()?;
        let n = tx.execute(
            "UPDATE pair_shares SET all_devices = ?2 WHERE pair_id = ?1",
            params![pair_id, all],
        )?;
        if n == 0 {
            return Err(StoreError::Refused(
                "This folder isn't shared yet. Share it first, then choose its devices.".into(),
            ));
        }
        tx.execute("DELETE FROM pair_share_devices WHERE pair_id = ?1", [pair_id])?;
        {
            let mut put = tx.prepare_cached(
                "INSERT OR IGNORE INTO pair_share_devices (pair_id, device_id) VALUES (?1, ?2)",
            )?;
            for d in devices {
                put.execute(params![pair_id, d])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Whether the shared pair `pair_id` may be synced with `device`, either way
    /// (docs/INTRODUCTIONS.md §3.2): the device is on the pair's list, or the pair is shared with
    /// all devices and the device is known here. The one rule for serving a peer and for choosing
    /// the peers a pair pulls from and tells of changes. False when the pair is not shared.
    pub fn share_allows(&self, pair_id: &str, device: &str) -> Result<bool> {
        Ok(self.shared_with(pair_id, device)?
            || (self.share_all_devices(pair_id)? == Some(true) && self.device_known(device)? == Some(true)))
    }

    /// Accepts `from`'s offer of `share` for `pair_id`, in one transaction: the pair is shared
    /// under that share with `from` only, never "all my devices" (§3.3; the person can widen it
    /// later), its route becomes `route` when given (a Drive pair also syncs with devices), and
    /// the offer is marked accepted. Refused when another pair here uses the share, or when the
    /// pair is already shared under another one: replacing it would cut off the devices syncing
    /// it (review of #28). Either everything happens or nothing does.
    pub fn accept_received_share(
        &mut self,
        pair_id: &str,
        share_id: &str,
        from: &str,
        route: Option<wasl_core::Route>,
    ) -> Result<()> {
        let tx = self.conn.transaction()?;
        let other_pair: Option<String> = tx
            .query_row(
                "SELECT pair_id FROM pair_shares WHERE share_id = ?1 AND pair_id <> ?2",
                params![share_id, pair_id],
                |r| r.get(0),
            )
            .optional()?;
        if other_pair.is_some() {
            return Err(StoreError::Refused(
                "Another pair here already syncs this folder. Open that pair to change how it syncs.".into(),
            ));
        }
        let current: Option<String> = tx
            .query_row(
                "SELECT share_id FROM pair_shares WHERE pair_id = ?1",
                [pair_id],
                |r| r.get(0),
            )
            .optional()?;
        if current.as_deref().is_some_and(|c| c != share_id) {
            return Err(StoreError::Refused(
                "This pair is already shared with other devices. Choose another folder for the offered one, or stop sharing this pair first.".into(),
            ));
        }
        tx.execute(
            "INSERT INTO pair_shares (pair_id, share_id, all_devices) VALUES (?1, ?2, 0)
             ON CONFLICT (pair_id) DO UPDATE SET all_devices = 0",
            params![pair_id, share_id],
        )?;
        tx.execute("DELETE FROM pair_share_devices WHERE pair_id = ?1", [pair_id])?;
        tx.execute(
            "INSERT INTO pair_share_devices (pair_id, device_id) VALUES (?1, ?2)",
            params![pair_id, from],
        )?;
        if let Some(route) = route {
            let route = match route {
                wasl_core::Route::Drive => "drive",
                wasl_core::Route::DriveAndDevices => "drive_and_devices",
                wasl_core::Route::Devices => "devices",
            };
            tx.execute(
                "UPDATE pairs SET route = ?2 WHERE id = ?1",
                params![pair_id, route],
            )?;
        }
        tx.execute(
            "UPDATE received_offers SET state = ?3 WHERE share = ?1 AND from_device = ?2",
            params![share_id, from, OfferState::Accepted.as_str()],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// The shared pairs that list `device` by name: (pair, share ID). What this device offers it.
    pub fn shares_offered_to(&self, device: &str) -> Result<Vec<(String, String)>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT s.pair_id, s.share_id FROM pair_shares s
             JOIN pair_share_devices d ON d.pair_id = s.pair_id
             WHERE d.device_id = ?1 ORDER BY s.pair_id",
        )?;
        let rows = stmt.query_map([device], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Files and bytes in this device's index of a pair (deleted entries and folders left out).
    pub fn index_totals(&self, pair_id: &str) -> Result<(u64, u64)> {
        let (files, bytes): (i64, i64) = self.conn.query_row(
            "SELECT COUNT(*), COALESCE(SUM(size), 0) FROM device_index
             WHERE pair_id = ?1 AND deleted = 0 AND is_dir = 0",
            [pair_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        Ok((to_u64(files), to_u64(bytes)))
    }

    /// Whether this device has any note on members yet (none before introductions first ran).
    pub fn has_device_notes(&self) -> Result<bool> {
        Ok(self
            .conn
            .query_row("SELECT 1 FROM device_notes LIMIT 1", [], |_| Ok(()))
            .optional()?
            .is_some())
    }

    /// Whether `device` is known here; `None` when there is no note on it.
    pub fn device_known(&self, device: &str) -> Result<Option<bool>> {
        Ok(self
            .conn
            .query_row(
                "SELECT known FROM device_notes WHERE device_id = ?1",
                [device],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Every note: (device, known).
    pub fn device_notes(&self) -> Result<Vec<(String, bool)>> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT device_id, known FROM device_notes ORDER BY device_id")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Sets the note on `device`.
    pub fn set_device_known(&self, device: &str, known: bool) -> Result<()> {
        self.conn.execute(
            "INSERT INTO device_notes (device_id, known) VALUES (?1, ?2)
             ON CONFLICT (device_id) DO UPDATE SET known = excluded.known",
            params![device, known],
        )?;
        Ok(())
    }

    /// Notes `device` as `known` unless it has a note already. True if a note was made.
    pub fn note_device(&self, device: &str, known: bool) -> Result<bool> {
        Ok(self.conn.execute(
            "INSERT OR IGNORE INTO device_notes (device_id, known) VALUES (?1, ?2)",
            params![device, known],
        )? > 0)
    }

    /// Every offer received, oldest first.
    pub fn received_offers(&self) -> Result<Vec<ReceivedOffer>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT share, from_device, name, files, bytes, first_seen, state FROM received_offers
             ORDER BY first_seen, share, from_device",
        )?;
        let mut rows = stmt.query([])?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            out.push(offer_row(r)?);
        }
        Ok(out)
    }

    /// The offer of `share` from `from`.
    pub fn received_offer(&self, share: &str, from: &str) -> Result<Option<ReceivedOffer>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT share, from_device, name, files, bytes, first_seen, state FROM received_offers
             WHERE share = ?1 AND from_device = ?2",
        )?;
        let mut rows = stmt.query(params![share, from])?;
        match rows.next()? {
            Some(r) => Ok(Some(offer_row(r)?)),
            None => Ok(None),
        }
    }

    /// Takes in what `from` offers now. `from` is the device the transport authenticated, never a
    /// value from the offer, and each offer is kept under (share, sender): a member offering
    /// another device's share ID adds its own row beside the real sender's, never in its place
    /// (security review 2026-10). New offers are added as `new`, first seen at `now` (this
    /// device's clock); known ones get the latest name and counts, their state and first sighting
    /// stay; offers from `from` it no longer makes are dropped unless accepted. True if anything
    /// changed.
    pub fn put_received_offers(&mut self, from: &str, offers: &[NewOffer], now: i64) -> Result<bool> {
        let tx = self.conn.transaction()?;
        let mut changed = 0;
        {
            let mut put = tx.prepare_cached(
                "INSERT INTO received_offers (share, from_device, name, files, bytes, first_seen, state)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'new')
                 ON CONFLICT (share, from_device) DO UPDATE SET
                   name = excluded.name, files = excluded.files, bytes = excluded.bytes
                 WHERE (received_offers.name, received_offers.files, received_offers.bytes)
                       IS NOT (excluded.name, excluded.files, excluded.bytes)",
            )?;
            for o in offers {
                changed += put.execute(params![
                    o.share,
                    from,
                    o.name,
                    to_i64(o.files),
                    to_i64(o.bytes),
                    now
                ])?;
            }
            let keep: HashSet<&str> = offers.iter().map(|o| o.share.as_str()).collect();
            let mut stmt = tx.prepare_cached(
                "SELECT share FROM received_offers WHERE from_device = ?1 AND state != 'accepted'",
            )?;
            let held: Vec<String> = stmt
                .query_map([from], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            let mut del =
                tx.prepare_cached("DELETE FROM received_offers WHERE share = ?1 AND from_device = ?2")?;
            for share in held.iter().filter(|s| !keep.contains(s.as_str())) {
                changed += del.execute(params![share, from])?;
            }
        }
        tx.commit()?;
        Ok(changed > 0)
    }

    /// Keeps at most `per_device` offers from each device, then at most `total` in all. Within a
    /// device, declined offers go before new ones, oldest first; accepted ones always stay. The
    /// total is cut from the devices holding the most offers, so one device that offers a lot
    /// cannot push out the others' offers (security review 2026-10).
    pub fn trim_received_offers(&mut self, per_device: usize, total: usize) -> Result<()> {
        let tx = self.conn.transaction()?;
        {
            // Per device: its count, and its removable offers in the order they go.
            let mut stmt = tx.prepare_cached(
                "SELECT share, from_device, state FROM received_offers
                 ORDER BY CASE state WHEN 'declined' THEN 0 ELSE 1 END, first_seen, share",
            )?;
            let rows: Vec<(String, String, String)> = stmt
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .collect::<rusqlite::Result<_>>()?;
            let mut held: HashMap<String, usize> = HashMap::new();
            let mut removable: HashMap<String, VecDeque<String>> = HashMap::new();
            for (share, from, state) in rows {
                *held.entry(from.clone()).or_default() += 1;
                if state != OfferState::Accepted.as_str() {
                    removable.entry(from).or_default().push_back(share);
                }
            }
            let mut del =
                tx.prepare_cached("DELETE FROM received_offers WHERE share = ?1 AND from_device = ?2")?;
            // 1. Each device down to `per_device`.
            for (from, n) in held.iter_mut() {
                let queue = removable.entry(from.clone()).or_default();
                while *n > per_device {
                    let Some(share) = queue.pop_front() else { break };
                    del.execute(params![share, from])?;
                    *n -= 1;
                }
            }
            // 2. The total, counted again, from the device holding the most each time.
            let mut all: usize = held.values().sum();
            while all > total {
                let Some(from) = held
                    .iter()
                    .filter(|(f, _)| removable.get(*f).is_some_and(|q| !q.is_empty()))
                    .max_by(|a, b| a.1.cmp(b.1).then_with(|| b.0.cmp(a.0)))
                    .map(|(f, _)| f.clone())
                else {
                    break;
                };
                let share = removable
                    .get_mut(&from)
                    .and_then(VecDeque::pop_front)
                    .expect("a device with removable offers");
                del.execute(params![share, from])?;
                *held.get_mut(&from).expect("counted") -= 1;
                all -= 1;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Sets the state of `from`'s offer of `share`. False if there is no such offer.
    pub fn set_offer_state(&self, share: &str, from: &str, state: OfferState) -> Result<bool> {
        Ok(self.conn.execute(
            "UPDATE received_offers SET state = ?3 WHERE share = ?1 AND from_device = ?2",
            params![share, from, state.as_str()],
        )? > 0)
    }
}

fn offer_row(r: &rusqlite::Row<'_>) -> Result<ReceivedOffer> {
    let state: String = r.get(6)?;
    Ok(ReceivedOffer {
        share: r.get(0)?,
        from_device: r.get(1)?,
        name: r.get(2)?,
        files: to_u64(r.get(3)?),
        bytes: to_u64(r.get(4)?),
        first_seen: r.get(5)?,
        state: OfferState::parse(&state)
            .ok_or_else(|| StoreError::Corrupt(format!("offer state {state}")))?,
    })
}
