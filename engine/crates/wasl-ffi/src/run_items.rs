//! The files a run left out, in words (issue #56): what the phone's history screen reads. The
//! engine words them once (`Engine::run_items`), the same as the daemon's `run_items` request.

use crate::{WaslEngine, WaslError};
use wasl_proto::{Lang, RunItem};

#[uniffi::export]
impl WaslEngine {
    /// The files a run left out or could not sync, worded in `lang`, each with the stable key,
    /// values and fix and what to do about it. Empty when the run left nothing out.
    pub fn run_items(&self, run_id: String, lang: Lang) -> Result<Vec<RunItem>, WaslError> {
        Ok(self.engine.run_items(&run_id, lang)?)
    }
}
