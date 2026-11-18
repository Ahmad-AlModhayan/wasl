//! The CLI's view of the engine's catalogue of status and error words (`wasl_proto::words`): the
//! language comes from the CLI's choice, and nothing here words anything itself.

use crate::i18n;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use wasl_proto::words::{self, Lang, Rendered, Values};

/// What the daemon has told this run about the machine: the folder roots and account ids a cause must
/// never show, however it is worded.
#[derive(Default)]
struct Known {
    roots: Vec<String>,
    secrets: Vec<String>,
}

static KNOWN: Mutex<Known> = Mutex::new(Known {
    roots: Vec::new(),
    secrets: Vec::new(),
});

fn known() -> std::sync::MutexGuard<'static, Known> {
    KNOWN.lock().unwrap_or_else(|p| p.into_inner())
}

/// Takes the daemon's accounts and pair folders into the redaction of every cause shown after this.
pub fn learn(status: &wasl_ipc::DaemonStatus) {
    let mut k = known();
    for a in &status.accounts {
        // An id too short to be one would also cut ordinary words out of a message.
        for s in [&a.id, &a.email] {
            if s.len() >= 4 && !k.secrets.contains(s) {
                k.secrets.push(s.clone());
            }
        }
    }
    for p in &status.pairs {
        let root = &p.pair.local_root;
        if !root.is_empty() && !k.roots.contains(root) {
            k.roots.push(root.clone());
        }
    }
}

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
    let k = known();
    let roots: Vec<&str> = k.roots.iter().map(String::as_str).collect();
    let secrets: Vec<&str> = k.secrets.iter().map(String::as_str).collect();
    words::render_with(key, values, lang(), raw, &roots, &secrets, &mut |line| {
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
    let k = known();
    let roots: Vec<&str> = k.roots.iter().map(String::as_str).collect();
    let secrets: Vec<&str> = k.secrets.iter().map(String::as_str).collect();
    words::redact_with(raw, &roots, &secrets)
}

/// Why a run on `folder` ended as it did: the words, and the redacted cause behind them. A
/// partial run counts the items that did not sync when its message says how many. Without a kind (an
/// older daemon) the run reads as one the catalogue can't name: the same words, language and log line.
pub fn run_cause(
    status_partial: bool,
    kind: Option<wasl_proto::ErrorKind>,
    raw: &str,
    folder: &str,
) -> Rendered {
    let values = Values::default().folder(folder);
    if status_partial {
        return match words::partial_count(raw) {
            Some(n) => say("run.partial", &values.count(n), Some(raw)),
            None => say("run.partial_some", &values, Some(raw)),
        };
    }
    let key = match kind {
        Some(kind) => words::classify(kind, raw),
        None => words::FALLBACK,
    };
    say(key, &values, Some(raw))
}

/// The words of an entry that takes no values (a status label).
pub fn text(key: &str) -> String {
    say(key, &Values::default(), None).text
}
