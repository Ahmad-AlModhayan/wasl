//! What device sync keeps per pair (docs/DEVICE_SYNC.md §4–5): the share ID, this device's
//! index of versions, how far each peer has been read, and the chunk table of local files.

use crate::{Result, Store, StoreError, to_i64, to_u64};
use rusqlite::{OptionalExtension, params};
use std::collections::BTreeMap;
use wasl_core::nway::{Index, IndexEntry, Version};
use wasl_core::vv::VersionVector;

/// An index entry with its sequence number: (key, version, seq).
pub type IndexRow = (String, Version, u64);

/// Where one chunk of a local file sits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkRow {
    pub key: String,
    pub offset: u64,
    pub len: u64,
}

impl Store {
    /// The share ID of a pair, if it is shared with devices.
    pub fn share_id(&self, pair_id: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT share_id FROM pair_shares WHERE pair_id = ?1",
                [pair_id],
                |r| r.get(0),
            )
            .optional()?)
    }

    pub fn set_share_id(&self, pair_id: &str, share_id: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO pair_shares (pair_id, share_id) VALUES (?1, ?2)
             ON CONFLICT (pair_id) DO UPDATE SET share_id = excluded.share_id",
            params![pair_id, share_id],
        )?;
        Ok(())
    }

    /// Stops sharing a pair with devices; its device index stays (sharing again resumes it). Its
    /// device list goes with the share: sharing again starts from "all my devices".
    pub fn remove_share_id(&self, pair_id: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM pair_shares WHERE pair_id = ?1", [pair_id])?;
        self.conn
            .execute("DELETE FROM pair_share_devices WHERE pair_id = ?1", [pair_id])?;
        Ok(())
    }

    /// The pair shared under `share_id`.
    pub fn pair_for_share(&self, share_id: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT pair_id FROM pair_shares WHERE share_id = ?1",
                [share_id],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Every (pair, share ID).
    pub fn shares(&self) -> Result<Vec<(String, String)>> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT pair_id, share_id FROM pair_shares ORDER BY pair_id")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// This device's index of a pair; `None` before the first scan.
    pub fn load_index(&self, pair_id: &str) -> Result<Option<Index>> {
        let meta: Option<(i64, i64)> = self
            .conn
            .query_row(
                "SELECT me, seq FROM device_index_meta WHERE pair_id = ?1",
                [pair_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some((me, seq)) = meta else {
            return Ok(None);
        };
        let mut stmt = self.conn.prepare_cached(
            "SELECT key, is_dir, deleted, hash, size, mtime, vv, author, seq, local_size, local_mtime
             FROM device_index WHERE pair_id = ?1",
        )?;
        let mut rows = stmt.query([pair_id])?;
        let mut entries = BTreeMap::new();
        while let Some(r) = rows.next()? {
            let key: String = r.get(0)?;
            let vv: String = r.get(6)?;
            let vv: VersionVector =
                serde_json::from_str(&vv).map_err(|e| StoreError::Corrupt(format!("vv of {key}: {e}")))?;
            let version = Version {
                is_dir: r.get(1)?,
                deleted: r.get(2)?,
                hash: r.get(3)?,
                size: to_u64(r.get(4)?),
                mtime: r.get(5)?,
                vv,
                author: r.get::<_, i64>(7)? as u64,
            };
            entries.insert(
                key,
                IndexEntry {
                    version,
                    seq: to_u64(r.get(8)?),
                    local_size: to_u64(r.get(9)?),
                    local_mtime: r.get(10)?,
                },
            );
        }
        Ok(Some(Index {
            me: me as u64,
            seq: to_u64(seq),
            entries,
        }))
    }

    /// Writes the index's counters and its entries at `only` (every entry when `None`), and
    /// deletes the entries at `removed` (pruned tombstones), in one transaction.
    pub fn save_index(
        &mut self,
        pair_id: &str,
        index: &Index,
        only: Option<&[String]>,
        removed: &[String],
    ) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO device_index_meta (pair_id, me, seq) VALUES (?1, ?2, ?3)
             ON CONFLICT (pair_id) DO UPDATE SET me = excluded.me, seq = excluded.seq",
            params![pair_id, index.me as i64, to_i64(index.seq)],
        )?;
        {
            let mut put = tx.prepare_cached(
                "INSERT INTO device_index
                   (pair_id, key, is_dir, deleted, hash, size, mtime, vv, author, seq, local_size, local_mtime)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                 ON CONFLICT (pair_id, key) DO UPDATE SET
                   is_dir = excluded.is_dir, deleted = excluded.deleted, hash = excluded.hash,
                   size = excluded.size, mtime = excluded.mtime, vv = excluded.vv,
                   author = excluded.author, seq = excluded.seq,
                   local_size = excluded.local_size, local_mtime = excluded.local_mtime",
            )?;
            let picked: Vec<(&String, &IndexEntry)> = match only {
                None => index.entries.iter().collect(),
                Some(keys) => keys
                    .iter()
                    .filter_map(|k| index.entries.get_key_value(k))
                    .collect(),
            };
            for (key, e) in picked {
                let v = &e.version;
                put.execute(params![
                    pair_id,
                    key,
                    v.is_dir,
                    v.deleted,
                    v.hash,
                    to_i64(v.size),
                    v.mtime,
                    serde_json::to_string(&v.vv).expect("a vector is JSON"),
                    v.author as i64,
                    to_i64(e.seq),
                    to_i64(e.local_size),
                    e.local_mtime,
                ])?;
            }
            let mut del = tx.prepare_cached("DELETE FROM device_index WHERE pair_id = ?1 AND key = ?2")?;
            for key in removed {
                del.execute(params![pair_id, key])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// How far this device has read `peer`'s index of a pair.
    pub fn peer_since(&self, pair_id: &str, peer: &str) -> Result<u64> {
        let since: Option<i64> = self
            .conn
            .query_row(
                "SELECT since FROM peer_progress WHERE pair_id = ?1 AND peer = ?2",
                params![pair_id, peer],
                |r| r.get(0),
            )
            .optional()?;
        Ok(since.map_or(0, to_u64))
    }

    pub fn set_peer_since(&self, pair_id: &str, peer: &str, since: u64) -> Result<()> {
        self.conn.execute(
            "INSERT INTO peer_progress (pair_id, peer, since) VALUES (?1, ?2, ?3)
             ON CONFLICT (pair_id, peer) DO UPDATE SET since = excluded.since",
            params![pair_id, peer, to_i64(since)],
        )?;
        Ok(())
    }

    /// Records the chunk list (wire form) of the local file at `key` and where its chunks sit.
    pub fn put_chunk_list(
        &mut self,
        pair_id: &str,
        key: &str,
        blake3: &[u8; 32],
        list: &[u8],
        chunks: &[([u8; 32], u64, u64)],
    ) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO chunk_lists (pair_id, key, blake3, list) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (pair_id, key) DO UPDATE SET blake3 = excluded.blake3, list = excluded.list",
            params![pair_id, key, &blake3[..], list],
        )?;
        tx.execute(
            "DELETE FROM chunks WHERE pair_id = ?1 AND key = ?2",
            params![pair_id, key],
        )?;
        {
            let mut put = tx.prepare_cached(
                "INSERT INTO chunks (pair_id, hash, key, offset, len) VALUES (?1, ?2, ?3, ?4, ?5)",
            )?;
            for (hash, offset, len) in chunks {
                put.execute(params![pair_id, &hash[..], key, to_i64(*offset), to_i64(*len)])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// The chunk list (wire form) of the local file at `key`, if its content is `blake3`.
    pub fn chunk_list(&self, pair_id: &str, key: &str, blake3: &[u8; 32]) -> Result<Option<Vec<u8>>> {
        Ok(self
            .conn
            .query_row(
                "SELECT list FROM chunk_lists WHERE pair_id = ?1 AND key = ?2 AND blake3 = ?3",
                params![pair_id, key, &blake3[..]],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Where a chunk sits among the pair's local files (any one place).
    pub fn locate_chunks(&self, pair_id: &str, hash: &[u8; 32]) -> Result<Vec<ChunkRow>> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT key, offset, len FROM chunks WHERE pair_id = ?1 AND hash = ?2 LIMIT 8")?;
        let rows = stmt.query_map(params![pair_id, &hash[..]], |r| {
            Ok(ChunkRow {
                key: r.get(0)?,
                offset: to_u64(r.get(1)?),
                len: to_u64(r.get(2)?),
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The local file at `key` was removed or replaced by content without a chunk list.
    pub fn forget_chunks(&mut self, pair_id: &str, key: &str) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "DELETE FROM chunk_lists WHERE pair_id = ?1 AND key = ?2",
            params![pair_id, key],
        )?;
        tx.execute(
            "DELETE FROM chunks WHERE pair_id = ?1 AND key = ?2",
            params![pair_id, key],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// The whole-file BLAKE3 recorded for `key`.
    pub fn chunked_blake3(&self, pair_id: &str, key: &str) -> Result<Option<[u8; 32]>> {
        let b: Option<Vec<u8>> = self
            .conn
            .query_row(
                "SELECT blake3 FROM chunk_lists WHERE pair_id = ?1 AND key = ?2",
                params![pair_id, key],
                |r| r.get(0),
            )
            .optional()?;
        Ok(b.and_then(|b| b.try_into().ok()))
    }

    /// Index entries of a pair with a sequence number above `since`, in sequence order, at most
    /// `limit`, and whether more follow (what a peer asks for, read straight from the table).
    pub fn index_changes_since(
        &self,
        pair_id: &str,
        since: u64,
        limit: usize,
    ) -> Result<(Vec<IndexRow>, bool)> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT key, is_dir, deleted, hash, size, mtime, vv, author, seq
             FROM device_index WHERE pair_id = ?1 AND seq > ?2 ORDER BY seq LIMIT ?3",
        )?;
        let mut rows = stmt.query(params![pair_id, to_i64(since), to_i64(limit as u64 + 1)])?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            let key: String = r.get(0)?;
            let vv: String = r.get(6)?;
            let vv: VersionVector =
                serde_json::from_str(&vv).map_err(|e| StoreError::Corrupt(format!("vv of {key}: {e}")))?;
            let version = Version {
                is_dir: r.get(1)?,
                deleted: r.get(2)?,
                hash: r.get(3)?,
                size: to_u64(r.get(4)?),
                mtime: r.get(5)?,
                vv,
                author: r.get::<_, i64>(7)? as u64,
            };
            out.push((key, version, to_u64(r.get(8)?)));
        }
        let more = out.len() > limit;
        out.truncate(limit);
        Ok((out, more))
    }

    /// A local file of the pair whose content is `blake3`, and its chunk list (wire form).
    pub fn chunk_list_by_content(
        &self,
        pair_id: &str,
        blake3: &[u8; 32],
    ) -> Result<Option<(String, Vec<u8>)>> {
        Ok(self
            .conn
            .query_row(
                "SELECT key, list FROM chunk_lists WHERE pair_id = ?1 AND blake3 = ?2 LIMIT 1",
                params![pair_id, &blake3[..]],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?)
    }
}
