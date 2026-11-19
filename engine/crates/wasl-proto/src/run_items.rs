//! The per-file events of a run, in words (issue #56): which files a run left out and what to do
//! about each. The engine words them once, in the caller's language, from the catalogue; the CLI and
//! the apps draw them, and an agent reads the stable key, values and fix, never the sentence.

use crate::store::RunEvent;
use crate::words::{self, Fix, Lang, Rendered};
use serde::{Deserialize, Serialize};

/// One file a run left out, or could not sync, in words.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct RunItem {
    /// When the engine recorded it, Unix milliseconds.
    pub at: i64,
    /// The engine's code for the event (`FILE_GONE`, `NAME_REFUSED`): stable, for an agent.
    pub action: String,
    /// The file's path inside the folder, never a path outside it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// What happened: the catalogue's key, the sentence, the values (the file's name and the
    /// folder's) and the suggested fix. Its Details are the redacted cause.
    pub words: Rendered,
    /// What the person can do about it, in the same language: the words of `words.fix`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fix_words: Option<String>,
}

/// The items among a run's events, in the order they happened: the events the catalogue words
/// ([`words::event_key`]); routine ones (uploads, notes) are not items. `folder` is the folder's
/// name, and `roots` and `secrets` are taken out of every cause ([`words::redact_with`]).
pub fn run_items(
    events: &[RunEvent],
    folder: &str,
    lang: Lang,
    roots: &[&str],
    secrets: &[&str],
) -> Vec<RunItem> {
    events
        .iter()
        .filter_map(|e| {
            let words = words::render_event(
                &e.action,
                e.key.as_deref(),
                folder,
                lang,
                Some(&e.message),
                roots,
                secrets,
            )?;
            Some(RunItem {
                at: e.at,
                action: e.action.clone(),
                path: e.key.clone(),
                fix_words: words.fix.map(|f| fix_words(f, lang).to_owned()),
                words,
            })
        })
        .collect()
}

/// What the person can do for each [`Fix`], as a sentence. Safe fixes say what a tap does; the rest
/// say where to act. Never asks the person to delete or move a file for Wasl.
pub fn fix_words(fix: Fix, lang: Lang) -> &'static str {
    let (en, ar) = match fix {
        Fix::Retry => ("Sync again to try once more.", "زامِن مجددًا للمحاولة مرة أخرى."),
        Fix::SignInAgain => (
            "Sign in to your Google account again.",
            "سجّل الدخول إلى حساب Google مجددًا.",
        ),
        Fix::OpenFolder => (
            "Open the folder and check it is there.",
            "افتح المجلد وتأكد من وجوده.",
        ),
        Fix::OpenStorageSettings => (
            "Free some space in this device's storage settings.",
            "حرّر بعض المساحة من إعدادات التخزين في هذا الجهاز.",
        ),
        Fix::OpenDriveStorage => (
            "Free some space in your Google Drive.",
            "حرّر بعض المساحة في Google Drive.",
        ),
        Fix::OpenToRename => (
            "Rename the file, then sync again. Wasl never renames your files.",
            "غيّر اسم الملف ثم زامِن مجددًا. لا يغيّر وصل أسماء ملفاتك أبدًا.",
        ),
        Fix::UnlockVault => ("Unlock the vault.", "افتح قفل الخزنة."),
        Fix::GrantAccessAgain => (
            "Give Wasl access to the folder again.",
            "امنح وصل صلاحية الوصول إلى المجلد مجددًا.",
        ),
        Fix::UpdateWasl => ("Update Wasl.", "حدّث وصل."),
        Fix::Review => (
            "Review what is waiting for your decision.",
            "راجع ما ينتظر قرارك.",
        ),
    };
    match lang {
        Lang::En => en,
        Lang::Ar => ar,
    }
}

/// Every fix with its words, for the reference document.
pub(crate) fn fixes_markdown() -> String {
    let mut s = String::from(
        "# Suggested fixes\n\n\
         The words of each `fix` a run item or a status carries (`fix_words` on a run item). The fix\n\
         is typed; an app maps it to a button, and the CLI and agents use these sentences.\n\n",
    );
    for fix in ALL_FIXES {
        s.push_str(&format!(
            "## `{}`\n\n- English: {}\n- Arabic: {}\n\n",
            words::fix_name(fix),
            fix_words(fix, Lang::En),
            fix_words(fix, Lang::Ar)
        ));
    }
    s
}

/// Every [`Fix`]. [`fix_words`] matches without a wildcard, so a new fix fails the build until it has
/// words; add it here too (a test walks this list).
pub(crate) const ALL_FIXES: [Fix; 10] = [
    Fix::Retry,
    Fix::SignInAgain,
    Fix::OpenFolder,
    Fix::OpenStorageSettings,
    Fix::OpenDriveStorage,
    Fix::OpenToRename,
    Fix::UnlockVault,
    Fix::GrantAccessAgain,
    Fix::UpdateWasl,
    Fix::Review,
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::EventLevel;

    fn event(action: &str, key: Option<&str>, message: &str) -> RunEvent {
        RunEvent {
            at: 5,
            level: EventLevel::Warn,
            action: action.into(),
            key: key.map(Into::into),
            message: message.into(),
            detail: None,
            bytes: 0,
        }
    }

    fn events() -> Vec<RunEvent> {
        vec![
            event("UPLOAD", Some("a.txt"), "uploaded"),
            event(
                words::EVENT_FILE_GONE,
                Some("Trips/photo.jpg"),
                "gone: /home/me/Photos/Trips/photo.jpg token=SECRETTOKEN",
            ),
            event("INFO", None, "Skipped 2 Google Docs"),
            event(words::EVENT_NAME_REFUSED, Some("CON.txt"), "reserved name"),
        ]
    }

    #[test]
    fn only_the_files_left_out_are_items_in_the_order_they_happened() {
        let items = run_items(&events(), "Photos", Lang::En, &["/home/me/Photos"], &[]);
        assert_eq!(2, items.len());
        assert_eq!("FILE_GONE", items[0].action);
        assert_eq!(Some("Trips/photo.jpg"), items[0].path.as_deref());
        assert_eq!("error.file_gone", items[0].words.key);
        assert_eq!(Some("photo.jpg"), items[0].words.values.file.as_deref());
        assert_eq!(Some("Photos"), items[0].words.values.folder.as_deref());
        assert_eq!("NAME_REFUSED", items[1].action);
        assert_eq!("error.name_refused", items[1].words.key);
        assert_eq!(Some(Fix::OpenToRename), items[1].words.fix);
        assert!(run_items(&[], "Photos", Lang::En, &[], &[]).is_empty());
    }

    #[test]
    fn each_item_says_what_to_do_in_the_callers_language() {
        for lang in [Lang::En, Lang::Ar] {
            for item in run_items(&events(), "Photos", lang, &[], &[]) {
                let fix = item.words.fix.expect("both entries suggest a fix");
                assert_eq!(Some(fix_words(fix, lang)), item.fix_words.as_deref());
            }
        }
        let en = run_items(&events(), "Photos", Lang::En, &[], &[]);
        let ar = run_items(&events(), "Photos", Lang::Ar, &[], &[]);
        assert_eq!(en[1].words.key, ar[1].words.key);
        assert_ne!(en[1].words.text, ar[1].words.text);
        assert_ne!(en[1].fix_words, ar[1].fix_words);
        assert!(ar[1].words.text.contains("CON.txt"));
    }

    #[test]
    fn the_cause_is_redacted_and_the_words_carry_no_path() {
        let items = run_items(&events(), "Photos", Lang::En, &["/home/me/Photos"], &[]);
        let all = serde_json::to_string(&items).unwrap();
        assert!(!all.contains("SECRETTOKEN"), "{all}");
        assert!(!all.contains("/home/me"), "{all}");
        assert!(!items[0].words.text.contains("Trips"), "{}", items[0].words.text);
    }

    #[test]
    fn json_has_stable_names_and_leaves_out_what_is_absent() {
        let items = run_items(&events(), "Photos", Lang::En, &[], &[]);
        let v = serde_json::to_value(&items[1]).unwrap();
        assert_eq!("NAME_REFUSED", v["action"]);
        assert_eq!("CON.txt", v["path"]);
        assert_eq!("error.name_refused", v["words"]["key"]);
        assert_eq!("open_to_rename", v["words"]["fix"]);
        assert_eq!("CON.txt", v["words"]["values"]["file"]);
        assert!(v["fix_words"].as_str().is_some_and(|t| !t.is_empty()), "{v}");
        let bare = RunItem {
            path: None,
            fix_words: None,
            ..items[1].clone()
        };
        let v = serde_json::to_value(bare).unwrap();
        assert!(v.get("path").is_none() && v.get("fix_words").is_none(), "{v}");
    }

    #[test]
    fn every_fix_has_words_in_both_languages() {
        let mut seen = std::collections::HashSet::new();
        for fix in ALL_FIXES {
            assert!(seen.insert(words::fix_name(fix)), "{fix:?} listed twice");
            for lang in [Lang::En, Lang::Ar] {
                assert!(!fix_words(fix, lang).trim().is_empty(), "{fix:?} {lang:?}");
            }
            assert_ne!(fix_words(fix, Lang::En), fix_words(fix, Lang::Ar), "{fix:?}");
        }
        assert_eq!(10, seen.len());
    }
}
