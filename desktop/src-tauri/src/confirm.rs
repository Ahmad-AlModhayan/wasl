//! `wasl-desktop --confirm-agent`: the confirmation an agent's changes need before they are
//! accepted (docs/AGENT_CHANGES.md §5, security review S3-1). `wasld` starts this mode itself
//! when someone asks to accept, writes what to show (with a token minted for this one changeset)
//! on stdin, and reads the answer on stdout. Only the person's click on Accept sends the token
//! back; nothing else on this computer ever sees it, so a program that can talk to `wasld`
//! cannot answer for the person.
//!
//! It runs before the window, the tray and the single-instance check: a plain native dialog, then
//! exit. Started by hand it shows nothing useful (no daemon reads its answer).
//!
//! Plainly: this is a deterrent against an agent silently accepting its own work through Wasl's
//! CLI or protocol, not a security boundary. A program running as the same user can click this
//! dialog through the OS's UI automation or input APIs, or read the token from this process's or
//! `wasld`'s memory. After a decline or a timeout `wasld` waits before it shows another dialog for
//! that changeset (1 minute, doubling to at most 30), so an agent asking in a loop cannot keep one
//! on screen until it is clicked out of weariness.

use serde::{Deserialize, Serialize};
use std::io::{BufRead, Read, Write};

/// What `wasld` sends (wasld `ConfirmAsk`; unknown fields are ignored). `Debug` leaves the
/// token out.
#[derive(Deserialize)]
pub struct Ask {
    pub changeset_id: String,
    pub token: String,
    pub code: String,
    pub agent: String,
    #[serde(default)]
    pub label: Option<String>,
    pub pair_name: String,
    #[serde(default)]
    pub scope: String,
    pub items: usize,
    pub asked_by: String,
    #[serde(default)]
    pub language: Option<String>,
}

impl std::fmt::Debug for Ask {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ask")
            .field("changeset_id", &self.changeset_id)
            .field("token", &"<redacted>")
            .field("code", &self.code)
            .field("agent", &self.agent)
            .field("label", &self.label)
            .field("pair_name", &self.pair_name)
            .field("scope", &self.scope)
            .field("items", &self.items)
            .field("asked_by", &self.asked_by)
            .field("language", &self.language)
            .finish()
    }
}

/// The answer `wasld` reads (wasld `ConfirmAnswer`). `Debug` leaves the token out.
#[derive(Serialize)]
struct Answer<'a> {
    changeset_id: &'a str,
    token: &'a str,
    accept: bool,
}

impl std::fmt::Debug for Answer<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Answer")
            .field("changeset_id", &self.changeset_id)
            .field("token", &"<redacted>")
            .field("accept", &self.accept)
            .finish()
    }
}

/// The dialog's words, in the person's language.
pub struct Words {
    pub title: String,
    pub body: String,
    pub accept: String,
    pub cancel: String,
}

/// Arabic when the person chose it in Wasl, or chose "system" on an Arabic system.
fn arabic(language: Option<&str>) -> bool {
    match language.map(|l| l.trim().to_ascii_lowercase()) {
        Some(l) if l.starts_with("ar") => true,
        Some(l) if l.starts_with("en") => false,
        _ => ["LC_ALL", "LANG"]
            .iter()
            .filter_map(|k| std::env::var(k).ok())
            .any(|v| v.to_ascii_lowercase().starts_with("ar")),
    }
}

/// Format and separator characters that change how text reads without showing (bidi overrides
/// and isolates, zero-width marks, line and paragraph separators): text from the changeset could
/// otherwise reorder the dialog's own words (security review 2026-10).
fn invisible(c: char) -> bool {
    matches!(c,
        '\u{00AD}' | '\u{061C}' | '\u{180E}' | '\u{200B}'..='\u{200F}' | '\u{2028}'..='\u{202E}'
        | '\u{2060}'..='\u{206F}' | '\u{FEFF}' | '\u{FFF9}'..='\u{FFFB}')
}

/// Keeps text from the changeset (agent and folder names, a label the agent chose) to one short
/// line, so it cannot pass itself off as the dialog's own words.
fn plain(s: &str, max: usize) -> String {
    let one: String = s.chars().filter(|&c| !c.is_control() && !invisible(c)).take(max).collect();
    if s.chars().count() > max {
        format!("{one}…")
    } else {
        one
    }
}

pub fn words(ask: &Ask) -> Words {
    let agent = plain(&ask.agent, 40);
    let folder = if ask.scope.is_empty() {
        plain(&ask.pair_name, 60)
    } else {
        format!("{} / {}", plain(&ask.pair_name, 40), plain(&ask.scope, 60))
    };
    let label = ask.label.as_deref().map(|l| plain(l, 80));
    let n = ask.items;
    let asked_by = plain(&ask.asked_by, 20);
    if arabic(ask.language.as_deref()) {
        let what = match &label {
            Some(l) => format!("\nالمهمة: {l}"),
            None => String::new(),
        };
        Words {
            title: "قبول تغييرات وكيل؟".into(),
            body: format!(
                "غيّر {agent} عدد {n} من العناصر في {folder}.{what}\n\n\
                 القبول يرسلها إلى Drive وإلى أجهزتك الأخرى. اقبل فقط إن كنت أنت من طلب ذلك، \
                 وتأكد أن الرمز هو {code}.\n\nالبرنامج الطالب يقول إنه: {asked_by} (لا يستطيع وصل التحقق من ذلك)",
                code = ask.code
            ),
            accept: "قبول".into(),
            cancel: "إلغاء".into(),
        }
    } else {
        let what = match &label {
            Some(l) => format!("\nTask: {l}"),
            None => String::new(),
        };
        Words {
            title: "Accept an agent's changes?".into(),
            body: format!(
                "{agent} changed {n} item{s} in {folder}.{what}\n\n\
                 Accepting sends them to Drive and your other devices. Accept only if you asked \
                 for this, and check that the code is {code}.\n\nThe program asking says it is: {asked_by} (Wasl can't verify this)",
                s = if n == 1 { "" } else { "s" },
                code = ask.code
            ),
            accept: "Accept".into(),
            cancel: "Cancel".into(),
        }
    }
}

/// Runs the confirmation; the process exit code.
pub fn run() -> i32 {
    let mut line = String::new();
    // One line from wasld, capped: this mode reads nothing else.
    if std::io::stdin()
        .lock()
        .take(64 << 10)
        .read_line(&mut line)
        .is_err()
    {
        return 2;
    }
    let Ok(ask) = serde_json::from_str::<Ask>(line.trim()) else {
        return 2;
    };
    let w = words(&ask);
    let choice = rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Warning)
        .set_title(&w.title)
        .set_description(&w.body)
        .set_buttons(rfd::MessageButtons::OkCancelCustom(w.accept.clone(), w.cancel))
        .show();
    let accept = match choice {
        rfd::MessageDialogResult::Custom(label) => label == w.accept,
        rfd::MessageDialogResult::Ok | rfd::MessageDialogResult::Yes => true,
        _ => false,
    };
    let answer = Answer {
        changeset_id: &ask.changeset_id,
        token: if accept { &ask.token } else { "" },
        accept,
    };
    let Ok(mut out) = serde_json::to_vec(&answer) else {
        return 2;
    };
    out.push(b'\n');
    let mut stdout = std::io::stdout().lock();
    if stdout.write_all(&out).and_then(|()| stdout.flush()).is_err() {
        return 2;
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Security review 2026-10: bidi overrides and zero-width marks from an agent's label could
    /// reorder the dialog's own words.
    #[test]
    fn invisible_and_bidi_characters_are_dropped() {
        let label = "fix\u{202E}tpecca\u{2066}x\u{2069}\u{200B}\u{2028}y";
        assert_eq!("fixtpeccaxy", plain(label, 40));
    }

    fn ask(language: Option<&str>) -> Ask {
        Ask {
            changeset_id: "cs".into(),
            token: "secret".into(),
            code: "042137".into(),
            agent: "claude-code".into(),
            label: Some("Fix the build\nAccept: safe".into()),
            pair_name: "Work".into(),
            scope: "src".into(),
            items: 3,
            asked_by: "cli".into(),
            language: language.map(Into::into),
        }
    }

    #[test]
    fn the_dialog_says_who_what_where_and_the_code_but_never_the_token() {
        let w = words(&ask(Some("en")));
        assert!(w.body.contains("claude-code changed 3 items in Work / src"), "{}", w.body);
        assert!(w.body.contains("042137"));
        assert!(w.body.contains("says it is: cli (Wasl can't verify this)"));
        assert!(!w.body.contains("secret"));
        // The agent's label stays on one line of its own.
        assert!(w.body.contains("Task: Fix the buildAccept: safe"), "{}", w.body);
        let ar = words(&ask(Some("ar")));
        assert_eq!("قبول", ar.accept);
        assert!(ar.body.contains("042137") && !ar.body.contains("secret"));
    }

    #[test]
    fn the_token_never_shows_in_debug_output() {
        let a = ask(Some("en"));
        assert!(!format!("{a:?}").contains("secret"));
        assert!(format!("{a:?}").contains("042137"));
        let answer = Answer {
            changeset_id: "cs",
            token: "secret",
            accept: true,
        };
        assert!(!format!("{answer:?}").contains("secret"));
    }

    #[test]
    fn long_names_are_cut() {
        assert_eq!("abc…", plain("abcdef", 3));
        assert_eq!("abc", plain("abc", 3));
    }

    #[test]
    fn the_answer_is_one_json_line_wasld_reads() {
        let a = Answer {
            changeset_id: "cs",
            token: "t",
            accept: true,
        };
        assert_eq!(
            r#"{"changeset_id":"cs","token":"t","accept":true}"#,
            serde_json::to_string(&a).unwrap()
        );
    }
}
