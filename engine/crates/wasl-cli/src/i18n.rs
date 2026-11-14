//! Every user-facing message, in English and Arabic (SRS NFR-8).
//!
//! Messages take positional arguments `{0}`, `{1}`…; [`t!`](crate::t) fills them. The language is
//! `WASL_LANG`, else the daemon's `language` setting, else `LC_ALL`/`LANG` starting with `ar`,
//! else English. Arabic follows the product glossary: a folder pair is ربط (روابط), never زوج;
//! sync مزامنة, upload رفع, download تنزيل, recycle bin سلة المحذوفات, conflict تعارض, preview
//! معاينة, account حساب, paused متوقف مؤقتًا, synced تمت المزامنة. Numbers stay in Western digits;
//! "Google Drive" and "Drive" stay untranslated.

use std::fmt::Display;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use wasl_proto::SyncMode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    En,
    Ar,
}

static LANG: AtomicU8 = AtomicU8::new(0);
static FORCED: AtomicBool = AtomicBool::new(false);

/// `ar`, `ar_SA.UTF-8`, `ar-EG` → Arabic; `en…` → English; anything else (`system`, `C`) → none.
pub fn parse(s: &str) -> Option<Lang> {
    let s = s.trim().to_ascii_lowercase();
    if s.starts_with("ar") {
        Some(Lang::Ar)
    } else if s.starts_with("en") {
        Some(Lang::En)
    } else {
        None
    }
}

/// The choice from the environment alone: the language, and whether `WASL_LANG` forced it.
pub fn choose(wasl_lang: Option<&str>, lc_all: Option<&str>, lang: Option<&str>) -> (Lang, bool) {
    if let Some(l) = wasl_lang.and_then(parse) {
        return (l, true);
    }
    let system = [lc_all, lang]
        .into_iter()
        .flatten()
        .find(|v| !v.trim().is_empty());
    let l = match system {
        Some(v) if v.trim().to_ascii_lowercase().starts_with("ar") => Lang::Ar,
        _ => Lang::En,
    };
    (l, false)
}

/// Reads `WASL_LANG`, `LC_ALL` and `LANG`.
pub fn init_from_env() {
    let var = |k: &str| std::env::var(k).ok();
    let (l, forced) = choose(
        var("WASL_LANG").as_deref(),
        var("LC_ALL").as_deref(),
        var("LANG").as_deref(),
    );
    set(l);
    FORCED.store(forced, Ordering::SeqCst);
}

/// Whether `WASL_LANG` chose the language (the daemon's setting then does not apply).
pub fn forced() -> bool {
    FORCED.load(Ordering::SeqCst)
}

/// Follows the daemon's `language` setting when it names a language.
pub fn apply_setting(value: &str) {
    if let Some(l) = parse(value) {
        set(l);
    }
}

pub fn set(l: Lang) {
    LANG.store(
        match l {
            Lang::En => 0,
            Lang::Ar => 1,
        },
        Ordering::SeqCst,
    );
}

pub fn current() -> Lang {
    if LANG.load(Ordering::SeqCst) == 1 {
        Lang::Ar
    } else {
        Lang::En
    }
}

/// Text from outside the program (file names from Drive or other devices, messages from the
/// engine) made safe for a terminal. Every control character (escape sequences, colour codes
/// included, and line breaks) and every invisible formatting character (bidirectional marks and
/// overrides, zero-width characters, line and paragraph separators, BOM) becomes `?`, so a
/// crafted name cannot move the cursor, retitle the window, hide text, fake an extra line or
/// change how a line reads. Only the program's own text carries colour or direction marks.
pub fn clean(text: &str) -> String {
    text.chars()
        .map(|c| {
            let invisible = matches!(
                c,
                '\u{061C}' | '\u{200B}'..='\u{200F}' | '\u{2028}'..='\u{202E}' | '\u{2060}'..='\u{2069}' | '\u{FEFF}'
            );
            if c.is_control() || invisible { '?' } else { c }
        })
        .collect()
}

/// Replaces `{0}`, `{1}`… in one pass (an argument containing `{1}` is left alone).
pub fn fill(template: &str, args: &[&dyn Display]) -> String {
    let mut out = String::with_capacity(template.len() + 16);
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let digits = after.bytes().take_while(u8::is_ascii_digit).count();
        if digits > 0 && after.as_bytes().get(digits) == Some(&b'}') {
            let i: usize = after[..digits].parse().unwrap_or(usize::MAX);
            match args.get(i) {
                // Arguments are data (names, messages): never terminal control.
                Some(a) => out.push_str(&clean(&a.to_string())),
                None => out.push_str(&rest[open..open + digits + 2]),
            }
            rest = &after[digits + 1..];
        } else {
            out.push('{');
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

/// A message in the current language: `t!(Key)` or `t!(Key, arg0, arg1…)`.
#[macro_export]
macro_rules! t {
    ($key:ident) => {
        $crate::i18n::M::$key.text($crate::i18n::current()).to_owned()
    };
    ($key:ident, $($arg:expr),+ $(,)?) => {
        $crate::i18n::fill(
            $crate::i18n::M::$key.text($crate::i18n::current()),
            &[$(&$arg as &dyn ::std::fmt::Display),+],
        )
    };
}

macro_rules! messages {
    ($($key:ident => $en:expr, $ar:expr;)*) => {
        /// The message keys.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum M { $($key),* }

        impl M {
            pub const ALL: &'static [M] = &[$(M::$key),*];

            pub fn text(self, lang: Lang) -> &'static str {
                match (self, lang) {
                    $((M::$key, Lang::En) => $en, (M::$key, Lang::Ar) => $ar,)*
                }
            }
        }
    };
}

messages! {
    ErrorLine => "Error: {0}", "خطأ: {0}";
    NotRunning => "wasld is not running. Start it with: wasl daemon start",
        "wasld لا يعمل. شغّله بالأمر: wasl daemon start";
    Untrusted => "The program answering is not this user's wasld.",
        "البرنامج الذي يجيب ليس wasld الخاص بهذا المستخدم.";
    WrongProtocol => "wasld speaks protocol {0}; this wasl speaks {1}. Update Wasl.",
        "wasld يستخدم البروتوكول {0} وهذا الأمر يستخدم {1}. حدّث وصل.";
    ConnectionLost => "Lost the connection to wasld: {0}", "انقطع الاتصال بـ wasld: {0}";
    NoSuchPair => "No folder pair matches \"{0}\". See: wasl pair list",
        "لا يوجد ربط يطابق \"{0}\". راجع: wasl pair list";
    AmbiguousPair => "\"{0}\" matches more than one folder pair; use its id:\n{1}",
        "\"{0}\" يطابق أكثر من ربط؛ استخدم المعرّف:\n{1}";

    MethodTwoWay => SyncMode::TwoWay.names().0, SyncMode::TwoWay.names().1;
    MethodUploadOnly => SyncMode::UploadOnly.names().0, SyncMode::UploadOnly.names().1;
    MethodUploadThenDelete => SyncMode::UploadThenDelete.names().0, SyncMode::UploadThenDelete.names().1;
    MethodMirrorUp => SyncMode::UploadMirror.names().0, SyncMode::UploadMirror.names().1;
    MethodDownloadOnly => SyncMode::DownloadOnly.names().0, SyncMode::DownloadOnly.names().1;
    MethodDownloadThenDelete => SyncMode::DownloadThenDelete.names().0, SyncMode::DownloadThenDelete.names().1;
    MethodMirrorDown => SyncMode::DownloadMirror.names().0, SyncMode::DownloadMirror.names().1;

    OverallUpToDate => "Synced", "تمت المزامنة";
    OverallSyncing => "Syncing", "تجري المزامنة";
    OverallPaused => "Paused", "متوقف مؤقتًا";
    OverallWaiting => "Waiting", "في الانتظار";
    OverallNeedsYou => "Needs you", "يحتاج إليك";
    OverallIdle => "No folder pairs yet", "لا توجد روابط بعد";

    PhaseStarting => "Starting", "يبدأ";
    PhaseListingRemote => "Reading Drive", "يقرأ Drive";
    PhaseScanningLocal => "Scanning the folder", "يفحص المجلد";
    PhasePlanning => "Planning", "يخطط";
    PhaseTransferring => "Transferring", "ينقل الملفات";
    PhaseDeleting => "Deleting", "يحذف";
    PhaseFinishing => "Finishing", "ينهي";

    RunRunning => "Running", "جارية";
    RunSuccess => "Done", "تمت";
    RunPartial => "Some files failed", "فشلت بعض الملفات";
    RunFailed => "Failed", "فشلت";
    RunBlocked => "Needs you", "يحتاج إليك";
    RunSkipped => "Did not start", "لم تبدأ";
    RunCancelled => "Cancelled", "أُلغيت";

    FilesOf => "{0}/{1} files", "{0}/{1} ملف";
    Checked => "{0} files checked", "فُحص {0} ملف";
    Changes => "{0} up, {1} down, {2} deleted", "رفع {0}، تنزيل {1}، حذف {2}";

    StateOn => "On", "يعمل";
    StateOff => "Off", "معطّل";
    StateSignIn => "Sign in again: {0}", "سجّل الدخول مجددًا: {0}";
    StateApprove => "Needs your approval", "ينتظر موافقتك";
    StateConflicts => "{0} conflicts", "تعارضات: {0}";
    StateWaitingPreview => "Waiting for preview", "بانتظار المعاينة";
    StateFailed => "Failed: {0}", "فشلت: {0}";
    StateNeverSynced => "Not synced yet", "لم تتم المزامنة بعد";

    StatusHeader => "Wasl (wasld {0}): {1}", "وصل (wasld {0}): {1}";
    AccountNeedsSignIn => "Account {0} must sign in again: wasl account add",
        "الحساب {0} يحتاج إلى تسجيل الدخول مجددًا: wasl account add";
    NoPairs => "No folder pairs yet. Add one with: wasl pair add <folder> <Drive path>",
        "لا توجد روابط بعد. أضف ربطًا بالأمر: wasl pair add <المجلد> <مسار Drive>";
    Never => "never", "لم تتم";

    ColId => "ID", "المعرّف";
    ColName => "NAME", "الاسم";
    ColFolders => "FOLDERS", "المجلدان";
    ColState => "STATE", "الحالة";
    ColLastSync => "LAST SYNC", "آخر مزامنة";
    ColMethod => "METHOD", "الطريقة";
    ColLocal => "LOCAL FOLDER", "المجلد المحلي";
    ColDrive => "DRIVE FOLDER", "مجلد Drive";
    ColAccount => "ACCOUNT", "الحساب";
    ColStarted => "STARTED", "البدء";
    ColPair => "PAIR", "الربط";
    ColResult => "RESULT", "النتيجة";
    ColChanges => "CHANGES", "التغييرات";
    ColMessage => "MESSAGE", "الرسالة";
    ColPath => "PATH", "المسار";
    ColDetected => "FOUND", "وقت الاكتشاف";
    ColWhere => "WHERE", "الموضع";
    ColDeleted => "DELETED", "وقت الحذف";
    ColSize => "SIZE", "الحجم";

    OutcomeSuccess => "{0}: synced ({1})", "{0}: تمت المزامنة ({1})";
    OutcomePartial => "{0}: synced, but some files failed: {1}", "{0}: تمت المزامنة لكن فشلت بعض الملفات: {1}";
    OutcomeFailed => "{0}: sync failed: {1}", "{0}: فشلت المزامنة: {1}";
    OutcomeBlocked => "{0}: stopped to ask you first: {1}", "{0}: توقفت المزامنة لتسألك أولًا: {1}";
    OutcomeSkipped => "{0}: did not start: {1}", "{0}: لم تبدأ المزامنة: {1}";
    OutcomeCancelled => "{0}: cancelled", "{0}: أُلغيت المزامنة";
    OutcomeRunning => "{0}: still running", "{0}: ما زالت المزامنة جارية";
    SyncStarted => "Sync started for {0}.", "بدأت مزامنة {0}.";
    SyncStartedAll => "Sync started for all folder pairs.", "بدأت مزامنة كل الروابط.";
    SyncFollow => "Follow it with: wasl status", "تابعها بالأمر: wasl status";
    SyncAllSummary => "{0} synced, {1} not synced.", "تمت مزامنة {0}، ولم تتم مزامنة {1}.";
    SeePreview => "See what it would do: wasl pair preview \"{0}\"",
        "اطّلع على ما ستفعله: wasl pair preview \"{0}\"";
    DaemonShuttingDown => "wasld is shutting down.", "wasld يتوقف عن العمل.";

    Paused => "Paused {0}.", "{0}: متوقف مؤقتًا.";
    Resumed => "Resumed {0}.", "{0}: استؤنفت المزامنة.";

    NoSuchFolder => "No such folder: {0}", "لا يوجد مجلد بهذا المسار: {0}";
    UnknownPreset => "No preset is called {0}. Presets: node, python, rust, android-gradle, flutter, docker, editors, os-junk", "لا توجد مجموعة قواعد باسم {0}. المجموعات: node و python و rust و android-gradle و flutter و docker و editors و os-junk";
    NoSuchAccount => "No signed-in account {0}. See: wasl account list",
        "لا يوجد حساب مسجّل باسم {0}. راجع: wasl account list";
    NoAccounts => "No Google account yet. Add one with: wasl account add",
        "لا يوجد حساب Google بعد. أضف حسابًا بالأمر: wasl account add";
    ChooseAccount => "More than one account is signed in; choose one with --account:\n{0}",
        "أكثر من حساب مسجّل؛ اختر واحدًا بالخيار --account:\n{0}";
    PairAdded => "Added {0}: {1} ({2}).", "أُضيف الربط {0}: {1} ({2}).";
    PairAddedNext => "It waits for you: see what the first sync will do, then start it: wasl pair preview \"{0}\" --start",
        "ينتظرك: اطّلع على ما ستفعله المزامنة الأولى ثم ابدأها: wasl pair preview \"{0}\" --start";
    StartWithPreview => "A new pair starts after its preview: wasl pair preview \"{0}\" --start",
        "يبدأ الربط الجديد بعد معاينته: wasl pair preview \"{0}\" --start";
    NeedsYes => "No terminal to ask on: add --yes to confirm.", "لا توجد طرفية للسؤال: أضف --yes للتأكيد.";
    YesNo => "[y/N]", "[نعم/لا]";
    RemoveWhat => "Folder pair {0}: {1}", "الربط {0}: {1}";
    FilesNotTouched => "Files on this computer and on Drive are not touched.",
        "لن تُمسّ الملفات على هذا الحاسوب ولا على Drive.";
    RemoveAsk => "Remove this folder pair?", "هل تريد إزالة هذا الربط؟";
    NothingChanged => "Nothing changed.", "لم يتغير شيء.";
    PairRemoved => "Removed {0}. Its files stay where they are.", "أُزيل الربط {0}. تبقى ملفاته في مكانها.";

    PreviewOf => "Preview of {0} ({1})", "معاينة {0} ({1})";
    SumUploads => "Upload to Drive", "رفع إلى Drive";
    SumDownloads => "Download from Drive", "تنزيل من Drive";
    SumDeleteLocal => "Delete on this computer", "حذف من هذا الحاسوب";
    SumDeleteRemote => "Delete on Drive", "حذف من Drive";
    SumMovedOffHere => "Then to this computer's bin", "ثم إلى سلة هذا الحاسوب";
    SumMovedOffDrive => "Then to Drive's bin", "ثم إلى سلة Drive";
    SumConflicts => "Conflicts", "تعارضات";
    SumUnchanged => "Unchanged", "دون تغيير";
    FilesSized => "{0} files ({1})", "{0} ملف ({1})";
    PreviewBlocked => "This sync stops to ask you first: {0}", "هذه المزامنة تتوقف لتسألك أولًا: {0}";
    Warning => "Warning: {0}", "تنبيه: {0}";
    Note => "Note: {0}", "ملاحظة: {0}";
    NothingToDo => "Nothing to do: both sides match.", "لا شيء لفعله: الجانبان متطابقان.";
    FirstChanges => "First {0} of {1} changes:", "أول {0} من {1} تغيير:";
    ActUpload => "upload", "رفع";
    ActDownload => "download", "تنزيل";
    ActExport => "export", "تصدير";
    ActDeleteLocal => "delete here", "حذف من هنا";
    ActDeleteRemote => "delete on Drive", "حذف من Drive";
    ActConflict => "conflict", "تعارض";
    ActMkdirLocal => "new folder here", "مجلد جديد هنا";
    ActMkdirRemote => "new folder on Drive", "مجلد جديد في Drive";
    ActSkip => "skip", "تخطٍّ";

    IgnoreNeedsExplain => "Use: wasl ignore --explain <pair> <path>",
        "الاستخدام: wasl ignore --explain <الربط> <المسار>";
    IsIgnored => "{0} is ignored.", "{0} مُتجاهَل.";
    NotIgnored => "{0} is not ignored: it syncs.", "{0} غير مُتجاهَل: تشمله المزامنة.";
    NotIgnoredHere => "{0} is not ignored on this device.", "{0} غير مُتجاهَل على هذا الجهاز.";
    IgnoreRule => "Rule: {0}", "القاعدة: {0}";
    IgnoreSource => "From: {0}", "المصدر: {0}";

    SignedIn => "Signed in", "مسجّل الدخول";
    SignInAgain => "Sign in again ({0})", "سجّل الدخول مجددًا ({0})";
    SignInBrowser => "Opening Google sign-in in your browser...", "جارٍ فتح تسجيل الدخول إلى Google في المتصفح...";
    SignInUrl => "If the browser didn't open, visit: {0}", "إن لم يُفتح المتصفح، افتح: {0}";
    AccountAdded => "Added account {0}.", "أُضيف الحساب {0}.";
    AccountFailed => "Sign-in failed: {0}", "فشل تسجيل الدخول: {0}";
    SignInTimeout => "Gave up waiting for sign-in after 5 minutes.", "انتهت مهلة انتظار تسجيل الدخول بعد 5 دقائق.";
    AccountRemoved => "Removed account {0}.", "أُزيل الحساب {0}.";

    NoHistory => "No sync runs yet.", "لا توجد عمليات مزامنة بعد.";
    NoProblems => "No problems.", "لا توجد مشكلات.";
    ProblemBlocked => "{0}: waiting for your approval. See: wasl pair preview \"{0}\"",
        "{0}: بانتظار موافقتك. راجع: wasl pair preview \"{0}\"";
    ProblemFailed => "{0}: last sync failed: {1}", "{0}: فشلت آخر مزامنة: {1}";
    ConflictsHeader => "Open conflicts: {0}", "تعارضات مفتوحة: {0}";
    ProblemAgentChanges => "Agent changes waiting for you: {0}. See: wasl agent list",
        "تغييرات وكلاء بانتظارك: {0}. راجع: wasl agent list";

    AgentWholePair => "the whole pair folder", "مجلد الربط كله";
    AgentBegun => "{0} is working in {1}. Changeset {2}.", "{0} يعمل في {1}. مجموعة التغييرات {2}.";
    AgentHeld => "{0} changed {1} item(s) in {2}. Held until you decide: wasl agent show {3}",
        "غيّر {0} عدد {1} من العناصر في {2}. محجوزة حتى تقرر: wasl agent show {3}";
    AgentAcceptInsideAgent => "Agents can't accept changes ({0} is set). Accept in the Wasl app or your own terminal.",
        "لا يقبل الوكلاء التغييرات ({0} مضبوط). اقبلها في تطبيق وصل أو في طرفيتك.";
    AgentConfirmInApp => "Confirm in the Wasl window that just opened (code {0}). Waiting for you…",
        "أكّد في نافذة وصل التي فُتحت الآن (الرمز {0}). بانتظارك…";
    AgentNotConfirmed => "Not accepted: the confirmation was declined or timed out. The changes still wait for you.",
        "لم تُقبل: رُفض التأكيد أو انتهت مهلته. ما زالت التغييرات بانتظارك.";
    AgentTryAgainIn => "The last confirmation for these changes was declined or timed out. Try again in {0} min.",
        "رُفض آخر تأكيد لهذه التغييرات أو انتهت مهلته. حاول مجددًا بعد {0} دقيقة.";
    AgentNothing => "The agent changed nothing.", "لم يغيّر الوكيل شيئًا.";
    AgentWatching => "Watching {0}. Each agent session snapshots it at its start.",
        "يُراقَب {0}. تلتقط كل جلسة وكيل نسخة منه عند بدئها.";
    AgentWatchedAlready => "{0} is watched already.", "{0} مُراقَب بالفعل.";
    AgentUnwatched => "No longer watching {0}. Earlier snapshots stay.",
        "توقفت مراقبة {0}. تبقى النسخ السابقة.";
    AgentNoWatches => "No folders are watched. Add one with wasl agent watch <folder>.",
        "لا توجد مجلدات مُراقَبة. أضف واحدًا بالأمر wasl agent watch <folder>.";
    AgentWatchDefaults => "Ignored by default: {0} (remove one with --unignore).",
        "متجاهَل افتراضيًا: {0} (أزل أيًّا منها بالخيار ‎--unignore).";
    AgentWatchNoDefaults => "No default ignores apply; .waslignore still does.",
        "لا تنطبق أي تجاهلات افتراضية؛ ويبقى ‎.waslignore ساريًا.";
    AgentNotADefault => "{0} is not one of this folder's default ignores ({1}).",
        "{0} ليس من التجاهلات الافتراضية لهذا المجلد ({1}).";
    AgentAccepted => "Accepted. The changes sync now.", "قُبلت. تُزامَن التغييرات الآن.";
    AgentReverted => "Put back {0} item(s). The agent's versions are in the recycle bin.",
        "أُعيد {0} عنصر. نسخ الوكيل في سلة المحذوفات.";
    AgentSkipped => "Left {0}: it changed after the session.", "تُرك {0}: تغيّر بعد الجلسة.";
    AgentRevertFailed => "Could not put back {0}: {1}", "تعذّرت إعادة {0}: {1}";
    AgentNone => "No agent changes are waiting.", "لا توجد تغييرات وكلاء بانتظارك.";
    AgentNoSuch => "No changeset {0}.", "لا توجد مجموعة تغييرات {0}.";
    AgentAmbiguous => "{0} matches more than one changeset; type more of its id.",
        "{0} يطابق أكثر من مجموعة تغييرات؛ اكتب جزءًا أطول من معرّفها.";
    AgentNoCommand => "Give the agent's command after --, e.g. wasl agent run --as gemini -- gemini",
        "اكتب أمر الوكيل بعد --، مثل: wasl agent run --as gemini -- gemini";
    AgentRunFailed => "Could not start {0}: {1}", "تعذّر تشغيل {0}: {1}";
    AgentHooksUnknown => "No hooks for {0}. Use: wasl agent run --as {0} -- <command>",
        "لا توجد خطافات لـ {0}. استخدم: wasl agent run --as {0} -- <الأمر>";
    AgentHooksWhere => "Add this to ~/.claude/settings.json (or the project's .claude/settings.json).",
        "أضف هذا إلى ~/.claude/settings.json (أو .claude/settings.json في المشروع).";
    AgentHookFailed => "wasl: agent hook: {0}", "wasl: خطاف الوكيل: {0}";
    AgentHookBadInput => "the hook's input is not JSON: {0}", "مدخل الخطاف ليس JSON: {0}";
    AgentHookUnknownEvent =>"unknown hook event {0}", "حدث خطاف غير معروف {0}";
    AgentStOpen => "working", "يعمل";
    AgentStHeld => "needs you", "بانتظارك";
    AgentStAccepted => "accepted", "مقبولة";
    AgentStReverted => "reverted", "مُعادة";
    AgentStPartly => "partly reverted", "مُعادة جزئيًا";
    AgentKAdded => "added", "أُضيف";
    AgentKChanged => "changed", "عُدّل";
    AgentKDeleted => "deleted", "حُذف";
    ColAgent => "AGENT", "الوكيل";
    ColChange => "CHANGE", "التغيير";
    OplogNone => "Nothing in the op-log matches.", "لا شيء في سجل العمليات يطابق.";
    OplogMore => "Showing the newest {0}; use --limit for more.", "تُعرض أحدث {0}؛ استخدم --limit للمزيد.";
    OplogPairAsId => "No pair named {0}; matching it as a pair id.",
        "لا يوجد ربط باسم {0}؛ سيُطابَق بوصفه معرّف ربط.";
    OplogBadStatus => "Unknown status {0}. Use one of: {1}", "حالة غير معروفة {0}. استخدم إحداها: {1}";
    OplogBadSince => "Can't read the time {0}. Use 2026-11-01, 2026-11-01T09:00:00Z, 30m, 24h or 7d.",
        "تعذّرت قراءة الوقت {0}. استخدم 2026-11-01 أو 2026-11-01T09:00:00Z أو 30m أو 24h أو 7d.";
    OplogStOpen => "open", "مفتوحة";
    OplogStStaged => "staged", "بانتظارك";
    OplogStCommitted => "committed", "معتمدة";
    OplogStDiscarded => "discarded", "مُهملة";
    OplogStReverted => "reverted", "مُعادة";
    OplogStPartly => "partly reverted", "مُعادة جزئيًا";
    OplogDecided => "Decided by {0} ({1}) at {2}.", "قرّرها {0} ({1}) في {2}.";
    ColActor => "ACTOR", "الفاعل";
    ColScope => "SCOPE", "النطاق";
    ColSeq => "SEQ", "الرقم";
    ColEntity => "ENTITY", "الكيان";

    BinEmptyAlready => "The recycle bin is empty.", "سلة المحذوفات فارغة.";
    ThisDevice => "This computer", "هذا الحاسوب";
    Restored => "Restored {0}.", "استُعيد {0}.";
    RestoreFailed => "Could not restore {0}: {1}", "تعذّرت استعادة {0}: {1}";
    EmptyAsk => "Delete everything in the recycle bin for good?", "هل تريد حذف كل ما في سلة المحذوفات نهائيًا؟";
    BinEmptied => "Deleted {0} items for good.", "حُذف {0} عنصر نهائيًا.";

    DaemonAlreadyRunning => "wasld is already running (version {0}).", "wasld يعمل بالفعل (الإصدار {0}).";
    DaemonNotFound => "Could not find wasld next to wasl or on PATH.", "تعذّر العثور على wasld بجانب wasl أو في PATH.";
    DaemonSpawnFailed => "Could not start {0}: {1}", "تعذّر تشغيل {0}: {1}";
    DaemonStarted => "wasld started (version {0}).", "بدأ wasld (الإصدار {0}).";
    DaemonExited => "wasld stopped right after starting ({0}). See its log.", "توقف wasld فور تشغيله ({0}). راجع سجله.";
    DaemonStartTimeout => "wasld did not answer within 10 seconds.", "لم يستجب wasld خلال 10 ثوانٍ.";
    DaemonStopped => "wasld stopped.", "توقف wasld.";
    DaemonStopTimeout => "wasld is still running after 10 seconds.", "ما زال wasld يعمل بعد 10 ثوانٍ.";
    DaemonRunning => "wasld is running (version {0}).", "wasld يعمل (الإصدار {0}).";

    ServiceFailed => "Could not change starting at sign-in: {0}", "تعذّر تغيير التشغيل عند تسجيل الدخول: {0}";
    ServiceInstalled => "wasld will start when you sign in ({0}).", "سيعمل wasld عند تسجيل دخولك ({0}).";
    ServiceUninstalled => "wasld will no longer start when you sign in.", "لن يعمل wasld بعد الآن عند تسجيل دخولك.";
    ServiceIsInstalled => "wasld starts when you sign in ({0}).", "يعمل wasld عند تسجيل دخولك ({0}).";
    ServiceNotInstalled => "wasld does not start when you sign in. Set it up with: wasl service install",
        "لا يعمل wasld عند تسجيل دخولك. فعّل ذلك بالأمر: wasl service install";

    DevicesOff => "Device sync is off on this computer.", "المزامنة بين الأجهزة متوقفة على هذا الحاسوب.";
    Connected => "connected", "متصل";
    NotConnected => "not connected", "غير متصل";
    PathLan => "same network", "الشبكة نفسها";
    PathDirect => "over the internet", "عبر الإنترنت";
    PathPunched => "over the internet, through both routers", "عبر الإنترنت، من خلال الموجّهين";
    ListeningOn => "Other devices reach this computer on port {0}.", "تصل الأجهزة الأخرى إلى هذا الحاسوب على المنفذ {0}.";
    OnlyThisDevice => "No other device yet. Add one with: wasl device add",
        "لا يوجد جهاز آخر بعد. أضف جهازًا بالأمر: wasl device add";
    PairShowCode => "On the new device, scan this code in Wasl, or run: wasl device join <code>",
        "على الجهاز الجديد، امسح هذا الرمز في وصل، أو نفّذ: wasl device join <الرمز>";
    PairCodeNext => "The code works once, for 10 minutes. Waiting for the other device…",
        "يعمل الرمز مرة واحدة لمدة 10 دقائق. بانتظار الجهاز الآخر…";
    PairReaching => "Reaching the other device…", "جارٍ الوصول إلى الجهاز الآخر…";
    PairWords => "Both devices now show four words:", "يعرض الجهازان الآن أربع كلمات:";
    PairWordsAsk => "Are they the same on the other device?", "هل هي نفسها على الجهاز الآخر؟";
    PairNotAdded => "Nothing was added. Make a new code to try again.",
        "لم يُضف شيء. أنشئ رمزًا جديدًا للمحاولة مرة أخرى.";
    PairWaitingOther => "Waiting for the other device to confirm…", "بانتظار تأكيد الجهاز الآخر…";
    PairDone => "Paired with {0}. Share a folder with it: wasl pair share <pair>",
        "تم الإقران مع {0}. شارك مجلدًا معه: wasl pair share <الربط>";
    NoSuchDevice => "No device {0} in your group. See: wasl device list",
        "لا يوجد جهاز {0} في مجموعتك. راجع: wasl device list";
    RemoveDeviceWhat => "Remove {0} ({1}) from your devices? It will never sync with them again; pairing it again gives it a new identity.",
        "إزالة {0} ({1}) من أجهزتك؟ لن يزامن معها مرة أخرى؛ وإقرانه من جديد يمنحه هوية جديدة.";
    DeviceRemoved => "Removed {0} from your devices.", "أُزيل {0} من أجهزتك.";
    DeviceConnected => "Connected to {0} at {1}.", "تم الاتصال بـ {0} على {1}.";
    PairShared => "{0} now syncs with your devices too.", "يُزامَن {0} مع أجهزتك أيضًا.";
    ShareCodeNext => "On another device, sync its folder with this one: wasl pair share <pair> --code {0}",
        "على جهاز آخر، زامن مجلده مع هذا المجلد: wasl pair share <الربط> --code {0}";
    PairJoinedShare => "{0} syncs with the same folder on your other devices.",
        "يُزامَن {0} مع المجلد نفسه على أجهزتك الأخرى.";
    PairUnshared => "{0} no longer syncs with your devices.", "لم يعد {0} يُزامَن مع أجهزتك.";
    DeviceNew => "new: not kept here yet", "جديد: لم يُعتمد هنا بعد";
    DeviceArrived => "{0} added {1} to your devices. Until you keep it, it gets no folder from this computer. Keep it: wasl device keep <device>. If it isn't yours, remove it: wasl device remove <device>",
        "أضاف {0} الجهاز {1} إلى أجهزتك. لن يصله أي مجلد من هذا الحاسوب حتى تعتمده. اعتمده: wasl device keep <الجهاز>. وإن لم يكن لك فأزله: wasl device remove <الجهاز>";
    DeviceKept => "{0} now gets the folders you share with all your devices.",
        "يصل {0} الآن إلى المجلدات التي تشاركها مع جميع أجهزتك.";
    ColFrom => "FROM", "من";
    ColFiles => "FILES", "الملفات";
    ColCode => "CODE", "الرمز";
    NoOffers => "No device offers this computer a folder.", "لا يعرض أي جهاز مجلدًا على هذا الحاسوب.";
    OfferSize => "{0} files, {1}", "{0} ملف، {1}";
    OfferDeclined => "declined", "مرفوض";
    OfferAccepted => "syncs here", "يُزامَن هنا";
    OffersNext => "Sync one with a folder here: wasl device accept <code> <folder>. Hide one: wasl device decline <code>",
        "لمزامنة أحدها مع مجلد هنا: wasl device accept <الرمز> <المجلد>. لإخفاء أحدها: wasl device decline <الرمز>";
    NoSuchOffer => "No offer matches \"{0}\". See: wasl device offers",
        "لا يوجد عرض يطابق \"{0}\". راجع: wasl device offers";
    AmbiguousOffer => "\"{0}\" matches more than one offer; use its code or --from:
{1}",
        "\"{0}\" يطابق أكثر من عرض؛ استخدم رمزه أو --from:
{1}";
    OfferAcceptedNext => "{0} will sync with {1}. Nothing moves until you have seen its first preview: wasl pair preview \"{2}\" --start",
        "سيُزامَن {0} مع {1}. لن يتغير شيء حتى ترى معاينته الأولى: wasl pair preview \"{2}\" --start";
    OfferDeclinedDone => "{0} from {1} is hidden on this computer.", "أُخفي {0} من {1} على هذا الحاسوب.";
    PreviewDevicesOnly => "{0} syncs with your devices only: its first sync takes what they have and sends what is here. Anything it replaces goes to the bin.",
        "يُزامَن {0} مع أجهزتك فقط: تأخذ مزامنته الأولى ما لديها وترسل ما هنا. وما يُستبدل يذهب إلى سلة المحذوفات.";
    PairOffered => "{0} is shared with and offered to: {1}. They accept it there (wasl device offers).",
        "يُزامَن {0} مع هذه الأجهزة ويُعرض عليها: {1}. يُقبل هناك (wasl device offers).";
    PinLater => "Files on demand live in views: wasl view add, then wasl view pin <path>",
        "الملفات عند الطلب في العروض: wasl view add ثم wasl view pin <المسار>";

    NoViews => "No views yet. Show a Drive folder here with: wasl view add <Drive path> <folder>",
        "لا توجد عروض بعد. اعرض مجلدًا من Drive هنا بالأمر: wasl view add <مسار Drive> <المجلد>";
    ViewsUnsupported => "Files on demand aren't available on this system yet.",
        "الملفات عند الطلب غير متاحة على هذا النظام بعد.";
    NoSuchView => "No view matches \"{0}\". See: wasl view list",
        "لا يوجد عرض يطابق \"{0}\". راجع: wasl view list";
    AmbiguousView => "\"{0}\" matches more than one view; use its id:\n{1}",
        "\"{0}\" يطابق أكثر من عرض؛ استخدم المعرّف:\n{1}";
    ColOnComputer => "ON THIS COMPUTER", "على هذا الحاسوب";
    CacheUse => "{0} of {1} ({2} kept)", "{0} من {1} (المحفوظ: {2})";
    ViewShowing => "{0} files, read {1}", "{0} ملف، قُرئ {1}";
    ViewStarting => "Starting", "يبدأ";
    ViewNotShowing => "Not showing: {0}", "لا يعرض الملفات: {0}";
    ViewLocalChanges => "{0}: {1} things in its folder were left alone (never uploaded, never deleted):",
        "{0}: تُرك {1} عنصر في مجلده كما هو (لا يُرفع ولا يُحذف أبدًا):";
    ChangeOnlyHere => "only here, not on Drive", "هنا فقط، وليس على Drive";
    ChangeChangedHere => "changed here; Drive keeps its version", "تغيّر هنا؛ يبقى على Drive إصداره";
    ChangeDiffers => "a different file than the one on Drive", "ملف مختلف عن الموجود على Drive";
    ChangeUnsupported => "Windows can't show it on demand", "لا يستطيع Windows عرضه عند الطلب";
    ChangeFailed => "couldn't be updated; tried again on the next refresh ({0})", "تعذّر تحديثه؛ تُعاد المحاولة في التحديث التالي ({0})";
    ChangeRestored => "changed here, so Drive's version was put back; the changed copy is kept in Wasl's data folder ({0})",
        "تغيّر هنا فأُعيد إصدار Drive؛ النسخة المعدّلة محفوظة في مجلد بيانات وصل ({0})";
    ViewAdded => "{0} shows {1} at {2}. Files download only when opened; nothing written there is uploaded.",
        "يعرض {0} المجلد {1} في {2}. تُنزَّل الملفات عند فتحها فقط، ولا يُرفع شيء يُكتب هناك.";
    ViewAddedNotYet => "It can't show its files yet: {0}", "تعذّر عرض ملفاته بعد: {0}";
    ViewRemoveWhat => "Remove the view {0} at {1}?", "إزالة العرض {0} في {1}؟";
    ViewRemoveFiles => "Files kept on this computer stay as ordinary files; the others go from this computer. Nothing on Drive changes.",
        "تبقى الملفات المحفوظة على هذا الحاسوب ملفات عادية، وتُزال البقية منه. لا يتغير شيء في Drive.";
    ViewRemoveKeep => "Every file is downloaded first and kept as an ordinary file. Nothing on Drive changes.",
        "تُنزَّل كل الملفات أولًا وتبقى ملفات عادية. لا يتغير شيء في Drive.";
    ViewRemoved => "Removed the view {0}.", "أُزيل العرض {0}.";
    ViewPinned => "{0} stays on this computer.", "سيبقى {0} على هذا الحاسوب.";
    ViewUnpinned => "{0} may leave this computer when space is needed; it stays on Drive.",
        "قد يُزال {0} من هذا الحاسوب عند الحاجة إلى المساحة، ويبقى في Drive.";
    ViewRefreshed => "{0} now shows what is on Drive.", "يعرض {0} الآن ما في Drive.";
    BadSize => "\"{0}\" isn't a size. Use e.g. 20G or 500M.", "\"{0}\" ليس حجمًا. استخدم مثلًا 20G أو 500M.";

    VaultNoManifest => "{0} has no vault.json. Download the whole vault folder from Drive.",
        "لا يحتوي {0} على vault.json. نزّل مجلد الخزنة كاملًا من Drive.";
    VaultBadManifest => "This vault cannot be read: {0}", "تعذّرت قراءة هذه الخزنة: {0}";
    VaultInfo => "Vault: {0} (format v{1}, created {2})", "الخزنة: {0} (الصيغة v{1}، أُنشئت {2})";
    VaultPassPrompt => "Vault passphrase: ", "عبارة مرور الخزنة: ";
    VaultNoTerminal => "No terminal to ask for the passphrase on: pipe it in with --passphrase-stdin.",
        "لا توجد طرفية لطلب عبارة المرور: مرّرها عبر --passphrase-stdin.";
    VaultPassEmpty => "The passphrase is empty.", "عبارة المرور فارغة.";
    VaultWrongPass => "Wrong passphrase for this vault.", "عبارة المرور غير صحيحة لهذه الخزنة.";
    VaultPassOk => "Passphrase: ok", "عبارة المرور: صحيحة";
    VaultDecrypting => "Decrypting into {0}", "جارٍ فك التشفير إلى {0}";
    VaultFileOk => "  ok      {0}", "  تم      {0}";
    VaultFileFailed => "  FAILED  {0}: {1}", "  فشل     {0}: {1}";
    VaultSummary => "{0} folders, {1} files, {2} decrypted.", "المجلدات: {0}، الملفات: {1}، فُكّ تشفير {2}.";
    VaultSkippedHeader => "Skipped {0} items:", "تُخطّي {0} عنصر:";
    VaultFailedHeader => "{0} items could not be decrypted:", "تعذّر فك تشفير {0} عنصر:";
    VaultItem => "  - {0}: {1}", "  - {0}: {1}";
    ReasonNotVaultItem => "not an item of this vault", "ليس من عناصر هذه الخزنة";
    ReasonExists => "already in the output folder (use --overwrite)",
        "موجود بالفعل في مجلد الإخراج (استخدم --overwrite)";
    ReasonSameName => "another item in this folder has the same name",
        "عنصر آخر في هذا المجلد يحمل الاسم نفسه";
    ReasonUnsafeName => "its name cannot be used on this computer", "لا يمكن استخدام اسمه على هذا الحاسوب";
    ReasonNoDirId => "the folder's .wasl-dir is missing or does not verify, so its contents cannot be named",
        "ملف .wasl-dir الخاص بالمجلد مفقود أو لا يمكن التحقق منه، فلا يمكن معرفة أسماء محتوياته";
    ReasonMissingChunk => "a piece of the file is missing: {0}", "جزء من الملف مفقود: {0}";
    ReasonDamaged => "modified or damaged: {0}", "معدَّل أو تالف: {0}";
    ReasonIo => "could not read or write: {0}", "تعذّرت القراءة أو الكتابة: {0}";

    NoVaults => "No vaults on this computer yet. Make one with: wasl vault create <Drive path>",
        "لا توجد خزائن على هذا الحاسوب بعد. أنشئ واحدة بالأمر: wasl vault create <مسار Drive>";
    NoSuchVault => "No vault matches \"{0}\". See: wasl vault list",
        "لا توجد خزنة تطابق \"{0}\". راجع: wasl vault list";
    AmbiguousVault => "\"{0}\" matches more than one vault; use its id:\n{1}",
        "\"{0}\" يطابق أكثر من خزنة؛ استخدم المعرّف:\n{1}";
    ColFormat => "FORMAT", "الصيغة";
    VaultUnlockedHere => "Unlocked on this computer", "مفتوحة على هذا الحاسوب";
    VaultLockedHere => "Locked", "مقفلة";
    VaultNeedsFolder => "Name the vault's own folder, e.g. \"My Drive/Private\".",
        "سمِّ مجلد الخزنة نفسه، مثل \"My Drive/Private\".";
    VaultLoseWarning => "If you lose the passphrase, this vault's files are lost: no one can recover them, not Google and not Wasl. Write it down and keep it somewhere safe.",
        "إن فقدت عبارة المرور، فُقدت ملفات هذه الخزنة. لا أحد يستطيع استعادتها: لا Google ولا وصل. اكتبها واحفظها في مكان آمن.";
    VaultPassNew => "New vault passphrase (at least 8 characters): ", "عبارة مرور الخزنة الجديدة (8 أحرف على الأقل): ";
    VaultPassRepeat => "Repeat the passphrase: ", "أعد إدخال عبارة المرور: ";
    VaultPassShort => "A vault's passphrase needs at least 8 characters. A sentence of several words is easy to remember and strong.",
        "تحتاج عبارة مرور الخزنة إلى 8 أحرف على الأقل. جملة من عدة كلمات سهلة التذكّر وقوية.";
    VaultPassMismatch => "The passphrases don't match.", "عبارتا المرور غير متطابقتين.";
    VaultMakingKeys => "Making the keys. This takes a few seconds on purpose.",
        "جارٍ إنشاء المفاتيح. يستغرق هذا بضع ثوانٍ عن قصد.";
    VaultChecking => "Checking the passphrase. This takes a few seconds on purpose.",
        "جارٍ التحقق من عبارة المرور. يستغرق هذا بضع ثوانٍ عن قصد.";
    VaultCreated => "Made the vault {0} in {1}. It is unlocked on this computer.\nSync a folder with it: wasl pair add <folder> \"{1}\"",
        "أُنشئت الخزنة {0} في {1}، وهي مفتوحة على هذا الحاسوب.\nزامن مجلدًا معها: wasl pair add <المجلد> \"{1}\"";
    VaultUnlocked => "{0} is unlocked on this computer: its pairs sync and its views show their files.",
        "فُتحت الخزنة {0} على هذا الحاسوب: تتزامن روابطها وتعرض عروضها ملفاتها.";
    VaultLocked => "{0} is locked on this computer: its pairs wait and its views show nothing until you unlock it. Nothing on Drive changed.",
        "أُقفلت الخزنة {0} على هذا الحاسوب: تنتظر روابطها ولا تعرض عروضها شيئًا حتى تفتحها. لم يتغير شيء في Drive.";
    VaultUpgrading => "Copying {0} into a new vault. Every file passes through this computer, so this takes a while; if it stops, run the same command again.", "يُنسخ {0} إلى خزنة جديدة. يمر كل ملف بهذا الحاسوب فيستغرق ذلك وقتًا؛ إن توقف فشغّل الأمر نفسه مرة أخرى.";
    VaultUpgraded => "Copied {0} into {1}: {2} files ({3}); {4} were already there.", "نُسخت {0} إلى {1}: {2} ملف ({3})؛ {4} كانت موجودة من قبل.";
    VaultNotCopied => "Not copied: {0}", "لم يُنسخ: {0}";
    VaultUpgradeNext => "The old vault and its pairs are unchanged. Next: add a pair or view on the new vault's folder, check it, then remove the old pair (docs/ENCRYPTION.md §11.12).", "لم تتغير الخزنة القديمة ولا روابطها. التالي: أضف ربطًا أو عرضًا على مجلد الخزنة الجديدة، وتحقق منه، ثم احذف الربط القديم (docs/ENCRYPTION.md §11.12).";
}

#[cfg(test)]
mod tests {
    use super::*;

    fn placeholders(s: &str) -> Vec<usize> {
        let mut v: Vec<usize> = s
            .match_indices('{')
            .filter_map(|(i, _)| {
                let rest = &s[i + 1..];
                let end = rest.find('}')?;
                rest[..end].parse().ok()
            })
            .collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    #[test]
    fn both_languages_take_the_same_arguments() {
        for m in M::ALL {
            assert_eq!(
                placeholders(m.text(Lang::En)),
                placeholders(m.text(Lang::Ar)),
                "{m:?}"
            );
        }
    }

    #[test]
    fn arabic_follows_the_glossary() {
        for m in M::ALL {
            let ar = m.text(Lang::Ar);
            assert!(!ar.contains("زوج"), "{m:?}: a folder pair is ربط");
            assert!(
                !ar.chars().any(|c| ('\u{0660}'..='\u{0669}').contains(&c)),
                "{m:?}: Western digits"
            );
            assert!(
                !ar.contains("جوجل") && !ar.contains("درايف"),
                "{m:?}: Drive stays untranslated"
            );
        }
    }

    #[test]
    fn fill_replaces_positions_once() {
        assert_eq!("a b a", fill("{0} {1} {0}", &[&"a", &"b"]));
        assert_eq!("x {1} y", fill("{0} y", &[&"x {1}", &"z"]));
        assert_eq!("{2} {x}", fill("{2} {x}", &[&1]));
    }

    #[test]
    fn language_choice() {
        assert_eq!((Lang::Ar, true), choose(Some("ar"), None, Some("en_US.UTF-8")));
        assert_eq!((Lang::En, true), choose(Some("en"), Some("ar_SA.UTF-8"), None));
        assert_eq!(
            (Lang::Ar, false),
            choose(None, Some("ar_EG.UTF-8"), Some("en_US"))
        );
        assert_eq!(
            (Lang::Ar, false),
            choose(Some("system"), None, Some("ar_SA.UTF-8"))
        );
        assert_eq!((Lang::En, false), choose(None, Some(""), Some("C.UTF-8")));
        assert_eq!((Lang::En, false), choose(None, None, None));
    }
}

#[cfg(test)]
mod clean_args {
    #[test]
    fn message_arguments_cannot_carry_terminal_control() {
        let s = super::fill("Uploaded {0}", &[&"x\u{1b}]52;c;Zm9v\u{7}.txt"]);
        assert_eq!("Uploaded x?]52;c;Zm9v?.txt", s);
    }
}
