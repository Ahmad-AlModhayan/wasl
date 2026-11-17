//! The CLI's view of the engine's catalogue of status and error words (`wasl_proto::words`): the
//! language comes from the CLI's choice, and nothing here words anything itself.

use crate::i18n;
use std::sync::atomic::{AtomicBool, Ordering};
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

/// The entry `key` in the current language, with its values, fix and Details.
pub fn say(key: &str, values: &Values, raw: Option<&str>) -> Rendered {
    words::render(key, values, lang(), raw, &mut |line| eprintln!("{line}"))
}

/// The words of an entry that takes no values (a status label).
pub fn text(key: &str) -> String {
    say(key, &Values::default(), None).text
}
