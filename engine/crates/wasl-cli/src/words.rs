//! The CLI's view of the engine's catalogue of status and error words (`wasl_proto::words`): the
//! language comes from the CLI's choice, and nothing here words anything itself.

use crate::i18n;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use wasl_proto::words::{self, Lang, Rendered, Values};

static DETAILS: AtomicBool = AtomicBool::new(false);

/// `--details`: print the cause behind an error.
pub fn set_details(on: bool) {
    DETAILS.store(on, Ordering::SeqCst);
}

pub fn details_wanted() -> bool {
    DETAILS.load(Ordering::SeqCst)
}

fn lang() -> Lang {
    match i18n::current() {
        i18n::Lang::En => Lang::En,
        i18n::Lang::Ar => Lang::Ar,
    }
}

/// The entry `key` in the current language, with its values, fix and Details. A cause the catalogue
/// can't name is logged to standard error once per run, however many folders show it.
pub fn say(key: &str, values: &Values, raw: Option<&str>) -> Rendered {
    static LOGGED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    words::render(key, values, lang(), raw, &[], &mut |line| {
        let fresh = LOGGED
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(line.clone());
        if fresh {
            eprintln!("{line}");
        }
    })
}

/// A cause shown without a run's words: the raw text with tokens and paths taken out.
pub fn plain(raw: &str) -> String {
    words::redact(raw, None, &[])
}

/// Why a run on `folder` ended as it did: the words, and the redacted cause behind them. A
/// partial run counts the items that did not sync. Without a kind (an older daemon), the cause itself.
pub fn run_cause(
    status_partial: bool,
    kind: Option<wasl_proto::ErrorKind>,
    raw: &str,
    folder: &str,
) -> Rendered {
    let values = Values::default().folder(folder);
    if status_partial {
        let n = words::partial_count(raw);
        return say("run.partial", &values.count(n), Some(raw));
    }
    match kind {
        Some(kind) => say(words::classify(kind, raw), &values, Some(raw)),
        None => Rendered {
            key: String::new(),
            text: plain(raw),
            fix: None,
            values,
            details: None,
        },
    }
}

/// The words of an entry that takes no values (a status label).
pub fn text(key: &str) -> String {
    say(key, &Values::default(), None).text
}
