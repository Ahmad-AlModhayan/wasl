//! The files a run left out, in words (issue #56): the engine reads the run's per-file events and
//! words them once, so the daemon, the phone binding and the command line all say the same thing.

use crate::engine::Engine;
use crate::error::{EngineError, Result};
use wasl_proto::{Lang, RunItem, run_items};

impl Engine {
    /// The files run `run_id` left out or could not sync, in `lang`, in the order they happened.
    /// Empty when the run left nothing out; [`EngineError::NotFound`] when there is no such run (a
    /// run older than the history's 90 days is gone). The causes behind the words are redacted of
    /// the folder's own path and of its account's id.
    pub fn run_items(&self, run_id: &str, lang: Lang) -> Result<Vec<RunItem>> {
        let s = self.store.lock();
        let run = s
            .run(run_id)?
            .ok_or_else(|| EngineError::NotFound(format!("run {run_id}")))?;
        let events = s.run_events(run_id)?;
        // A pair removed since still has its history until pruned; its name is then not known.
        let pair = s.get_pair(&run.pair_id)?;
        drop(s);
        let (name, root, account) = match &pair {
            Some(p) => (p.name.as_str(), p.local_root.as_str(), p.account_id.as_deref()),
            None => (run.pair_id.as_str(), "", None),
        };
        let roots: Vec<&str> = Some(root).filter(|r| !r.is_empty()).into_iter().collect();
        let secrets: Vec<&str> = account.into_iter().collect();
        Ok(run_items(&events, name, lang, &roots, &secrets))
    }
}
