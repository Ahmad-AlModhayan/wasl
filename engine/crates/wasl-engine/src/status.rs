//! Each folder's status, decided once (issue #29): the engine gathers what it knows about a pair
//! (its record, its latest runs, its open conflicts, its account, what holds the next run on this
//! device) and hands it to the one rule set in `wasl_proto::status`. The daemon and the phone binding
//! return the result; no app decides a folder's state itself.

use crate::engine::Engine;
use crate::error::Result;
use wasl_core::conditions::blocker;
use wasl_proto::status::{Facts, LAST_RUNS, folder_status};
use wasl_proto::{FolderStatus, Lang, Pair};

impl Engine {
    /// The status of every pair, in the language `lang`. `free_bytes` tells how much room the drive
    /// holding a pair's folder has (the engine can't read a drive; the platform can). A cause the
    /// catalogue can't name goes to `log` once, so it can be added to the catalogue.
    pub fn folder_statuses(
        &self,
        lang: Lang,
        free_bytes: &dyn Fn(&Pair) -> Option<u64>,
        log: &mut dyn FnMut(String),
    ) -> Result<Vec<FolderStatus>> {
        let (pairs, signed_out) = {
            let s = self.store.lock();
            let signed_out: Vec<String> = s
                .accounts()?
                .into_iter()
                .filter(|(_, _, auth_error)| auth_error.is_some())
                .map(|(id, _, _)| id)
                .collect();
            (s.list_pairs()?, signed_out)
        };
        let device = self.env.device_state();
        let mut out = Vec::with_capacity(pairs.len());
        for pair in &pairs {
            let (open_conflicts, skipped, runs) = {
                let s = self.store.lock();
                (
                    s.open_conflicts(Some(&pair.id))?.len() as u32,
                    s.last_skipped(&pair.id)?,
                    s.runs(Some(&pair.id), LAST_RUNS as u32)?,
                )
            };
            let facts = Facts {
                pair,
                running: self.is_running(&pair.id),
                open_conflicts,
                signed_out: pair.account_id.as_ref().is_some_and(|a| signed_out.contains(a)),
                blocker: blocker(&pair.conditions, &device, false),
                free_bytes: free_bytes(pair),
                skipped,
                runs: &runs,
            };
            out.push(folder_status(&facts, lang, log));
        }
        Ok(out)
    }
}
