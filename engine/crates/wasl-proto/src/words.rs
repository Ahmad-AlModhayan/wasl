//! The catalogue of status and error words (ADR-0003, `docs/STATUS_WORDS.md`).
//!
//! Every sentence Wasl shows about a status or an error lives here, in English and Arabic. An entry
//! has a stable key (the part scripts and agents read), the words with plural forms, the typed values
//! its words take, one suggested fix as a typed action, and a translator's note. [`render`] turns an
//! entry and its values into words in the caller's language; apps and the CLI keep no wording of their own.

use serde::{Deserialize, Serialize};

/// The languages the catalogue speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lang {
    En,
    Ar,
}

impl Lang {
    /// `ar`, `ar_SA.UTF-8`, `ar-EG` give Arabic and `en…` gives English. Anything else gives `None`;
    /// callers fall back to English.
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
}

/// The one thing an entry suggests doing. Safe, reversible fixes run on one tap; the rest only open
/// the place to act. Never renames, deletes or moves a file for the person.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Fix {
    Retry,
    SignInAgain,
    OpenFolder,
    OpenStorageSettings,
    OpenDriveStorage,
    OpenToRename,
    GrantAccessAgain,
    UpdateWasl,
}

/// A value an entry's words take.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    Folder,
    File,
    Device,
    Count,
}

impl Slot {
    fn name(self) -> &'static str {
        match self {
            Slot::Folder => "folder",
            Slot::File => "file",
            Slot::Device => "device",
            Slot::Count => "count",
        }
    }
}

/// The typed values a rendered entry was filled with, kept beside the words so an agent never
/// parses a sentence. Absent values read as a plain stand-in in the words ("this folder").
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Values {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<u64>,
}

impl Values {
    pub fn folder(mut self, v: impl Into<String>) -> Values {
        self.folder = Some(v.into());
        self
    }
    pub fn file(mut self, v: impl Into<String>) -> Values {
        self.file = Some(v.into());
        self
    }
    pub fn device(mut self, v: impl Into<String>) -> Values {
        self.device = Some(v.into());
        self
    }
    pub fn count(mut self, v: u64) -> Values {
        self.count = Some(v);
        self
    }
}

/// One language's words for an entry: the plural forms it uses. `other` is always there.
#[derive(Debug, Clone, Copy)]
pub struct Forms {
    pub zero: Option<&'static str>,
    pub one: Option<&'static str>,
    pub two: Option<&'static str>,
    pub few: Option<&'static str>,
    pub many: Option<&'static str>,
    pub other: &'static str,
}

impl Forms {
    /// A sentence with no plural forms.
    pub const fn single(other: &'static str) -> Forms {
        Forms {
            zero: None,
            one: None,
            two: None,
            few: None,
            many: None,
            other,
        }
    }
    /// English: one and other.
    pub const fn one_other(one: &'static str, other: &'static str) -> Forms {
        Forms {
            zero: None,
            one: Some(one),
            two: None,
            few: None,
            many: None,
            other,
        }
    }
    /// Arabic: zero, one, two, few, many and other.
    pub const fn six(
        zero: &'static str,
        one: &'static str,
        two: &'static str,
        few: &'static str,
        many: &'static str,
        other: &'static str,
    ) -> Forms {
        Forms {
            zero: Some(zero),
            one: Some(one),
            two: Some(two),
            few: Some(few),
            many: Some(many),
            other,
        }
    }

    fn pick(&self, cat: Plural) -> &'static str {
        let chosen = match cat {
            Plural::Zero => self.zero,
            Plural::One => self.one,
            Plural::Two => self.two,
            Plural::Few => self.few,
            Plural::Many => self.many,
            Plural::Other => None,
        };
        chosen.unwrap_or(self.other)
    }

    #[cfg(test)]
    fn all(&self) -> impl Iterator<Item = &'static str> {
        [
            self.zero,
            self.one,
            self.two,
            self.few,
            self.many,
            Some(self.other),
        ]
        .into_iter()
        .flatten()
    }
}

/// CLDR plural categories.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Plural {
    Zero,
    One,
    Two,
    Few,
    Many,
    Other,
}

/// The plural category of `n` in `lang`.
pub fn plural(lang: Lang, n: u64) -> Plural {
    match lang {
        Lang::En => {
            if n == 1 {
                Plural::One
            } else {
                Plural::Other
            }
        }
        Lang::Ar => match n {
            0 => Plural::Zero,
            1 => Plural::One,
            2 => Plural::Two,
            _ => match n % 100 {
                3..=10 => Plural::Few,
                11..=99 => Plural::Many,
                _ => Plural::Other,
            },
        },
    }
}

/// One catalogue entry.
#[derive(Debug, Clone, Copy)]
pub struct Entry {
    /// Stable and documented; never reworded once shipped.
    pub key: &'static str,
    pub en: Forms,
    pub ar: Forms,
    /// For translators: where this shows and what the values are.
    pub note: &'static str,
    pub fix: Option<Fix>,
    /// The values the words take.
    pub slots: &'static [Slot],
}

/// The key of the entry an unknown error gets.
pub const FALLBACK: &str = "error.unknown";

const fn e(
    key: &'static str,
    en: Forms,
    ar: Forms,
    fix: Option<Fix>,
    slots: &'static [Slot],
    note: &'static str,
) -> Entry {
    Entry {
        key,
        en,
        ar,
        note,
        fix,
        slots,
    }
}

use Fix::*;
use Slot::{Count, File, Folder};

/// Every entry. Keep the keys unique; `docs/STATUS_WORDS.md` is generated from this table.
pub const ENTRIES: &[Entry] = &[
    // ---- the whole-app status
    e(
        "overall.synced",
        Forms::single("Synced"),
        Forms::single("تمت المزامنة"),
        None,
        &[],
        "Short status of the whole app when every folder is up to date.",
    ),
    e(
        "overall.syncing",
        Forms::single("Syncing"),
        Forms::single("تجري المزامنة"),
        None,
        &[],
        "Short status of the whole app while any folder is syncing.",
    ),
    e(
        "overall.paused",
        Forms::single("Paused"),
        Forms::single("متوقف مؤقتًا"),
        None,
        &[],
        "Short status of the whole app when syncing is paused.",
    ),
    e(
        "overall.waiting",
        Forms::single("Waiting"),
        Forms::single("في الانتظار"),
        None,
        &[],
        "Short status of the whole app when folders wait (no network, a schedule, a first preview).",
    ),
    e(
        "overall.needs_you",
        Forms::single("Needs you"),
        Forms::single("يحتاج إليك"),
        None,
        &[],
        "Short status of the whole app when something needs the person.",
    ),
    e(
        "overall.idle",
        Forms::single("No folders yet"),
        Forms::single("لا توجد مجلدات بعد"),
        None,
        &[],
        "Short status of the whole app before any folder is added.",
    ),
    e(
        "status.folders_need_you",
        Forms::one_other("{count} folder needs you", "{count} folders need you"),
        Forms::six(
            "لا مجلدات تحتاج إليك",
            "مجلد واحد يحتاج إليك",
            "مجلدان يحتاجان إليك",
            "{count} مجلدات تحتاج إليك",
            "{count} مجلدًا يحتاج إليك",
            "{count} مجلد يحتاج إليك",
        ),
        None,
        &[Count],
        "Badge or heading on the home screen. {count} is the number of folders that need the person.",
    ),
    // ---- errors a run can end with (one per coarse error kind)
    e(
        "error.auth",
        Forms::single("Your Google sign-in has ended, so {folder} can't sync. Nothing was changed."),
        Forms::single("انتهى تسجيل دخولك إلى Google، لذلك لا يمكن مزامنة {folder}. لم يتغير شيء."),
        Some(SignInAgain),
        &[Folder],
        "Shown on a folder or in a notification when Google no longer accepts the saved sign-in.",
    ),
    e(
        "error.permission",
        Forms::single("Wasl isn't allowed to change {file} in {folder}. Skipped it; the rest synced."),
        Forms::single("لا يملك وصل إذن تعديل {file} في {folder}. تم تخطيه ومزامنة الباقي."),
        Some(OpenFolder),
        &[File, Folder],
        "A file or folder refused Wasl access. {file} is one file name; {folder} is the synced folder.",
    ),
    e(
        "error.quota",
        Forms::single("Google Drive is full, so {folder} can't send more files. Nothing was lost."),
        Forms::single("مساحة Google Drive ممتلئة، لذلك لا يستطيع {folder} رفع المزيد من الملفات. لم يضع شيء."),
        Some(OpenDriveStorage),
        &[Folder],
        "The person's Drive storage is full (not this device's disk).",
    ),
    e(
        "error.not_found",
        Forms::single("{file} isn't there any more. Skipped it; the rest of {folder} synced."),
        Forms::single("{file} لم يعد موجودًا. تم تخطيه ومزامنة بقية {folder}."),
        Some(Retry),
        &[File, Folder],
        "A file or folder Wasl was going to use isn't there.",
    ),
    e(
        "error.locked",
        Forms::single(
            "{file} is open in another program, so Wasl left it for now. It will be tried again at the next sync.",
        ),
        Forms::single(
            "{file} مفتوح في برنامج آخر، لذلك تركه وصل الآن. ستتم محاولته مجددًا في المزامنة التالية.",
        ),
        Some(Retry),
        &[File],
        "Another program holds the file open or locked.",
    ),
    e(
        "error.network",
        Forms::single("Wasl can't reach Google Drive right now. It will try again by itself."),
        Forms::single("لا يستطيع وصل الوصول إلى Google Drive الآن. سيحاول مجددًا من تلقاء نفسه."),
        Some(Retry),
        &[],
        "Shown while the network is down or flaky. A passing failure; only becomes 'needs you' when retries give up.",
    ),
    e(
        "error.rate_limit",
        Forms::single("Google Drive asked Wasl to slow down. It will try again in a moment."),
        Forms::single("طلب Google Drive من وصل أن يبطئ. سيحاول مجددًا بعد قليل."),
        Some(Retry),
        &[],
        "Google answered with 'too many requests'.",
    ),
    e(
        "error.server",
        Forms::single("Google Drive had a problem on its side. Wasl will try again at the next sync."),
        Forms::single("واجه Google Drive مشكلة من جانبه. سيحاول وصل مجددًا في المزامنة التالية."),
        Some(Retry),
        &[],
        "Google answered with a server error.",
    ),
    e(
        "error.io",
        Forms::single("Wasl couldn't read or write a file in {folder}. Skipped it; the rest synced."),
        Forms::single("تعذّر على وصل قراءة ملف في {folder} أو الكتابة فيه. تم تخطيه ومزامنة الباقي."),
        Some(OpenFolder),
        &[Folder],
        "A local file operation failed for a reason Wasl can't name more exactly.",
    ),
    e(
        "error.integrity",
        Forms::single(
            "A file in {folder} didn't arrive intact, so Wasl didn't keep it. It will be tried again.",
        ),
        Forms::single("لم يصل ملف في {folder} سليمًا، لذلك لم يحتفظ به وصل. ستتم محاولته مجددًا."),
        Some(Retry),
        &[Folder],
        "A downloaded or received file failed its check.",
    ),
    // ---- errors the daemon answers a request with
    e(
        "request.not_found",
        Forms::single("Wasl couldn't find what you asked for."),
        Forms::single("تعذّر على وصل العثور على ما طلبته."),
        None,
        &[],
        "The engine has no folder, device, file or vault by that name. Details carries the engine's own message.",
    ),
    e(
        "request.permission",
        Forms::single("Wasl isn't allowed to do that."),
        Forms::single("لا يملك وصل الإذن لتنفيذ ذلك."),
        None,
        &[],
        "The engine refused a request the person isn't allowed to make. Details carries the engine's own message.",
    ),
    e(
        "request.locked",
        Forms::single("That is in use right now, so Wasl can't do it yet."),
        Forms::single("هذا قيد الاستخدام الآن، لذلك لا يستطيع وصل تنفيذه بعد."),
        Some(Retry),
        &[],
        "The engine can't do a request because something is held open or locked. Details carries the engine's message.",
    ),
    e(
        "error.file_gone",
        Forms::single("{file} was gone before Wasl could sync it. Skipped it; the rest of {folder} synced."),
        Forms::single("اختفى {file} قبل أن يتمكن وصل من مزامنته. تم تخطيه ومزامنة بقية {folder}."),
        Some(Retry),
        &[File, Folder],
        "A file was deleted or moved between Wasl listing it and reading it. Not a problem; the next sync is clean.",
    ),
    e(
        "error.disk_full",
        Forms::single(
            "This device's storage is full, so Wasl couldn't save {file} in {folder}. Nothing was lost.",
        ),
        Forms::single(
            "مساحة التخزين في هذا الجهاز ممتلئة، لذلك تعذّر على وصل حفظ {file} في {folder}. لم يضع شيء.",
        ),
        Some(OpenStorageSettings),
        &[File, Folder],
        "The disk or phone storage on THIS device is full (contrast error.quota: the person's Drive is full).",
    ),
    e(
        "error.folder_missing",
        Forms::single("The folder {folder} isn't on this device, so Wasl didn't sync it."),
        Forms::single("المجلد {folder} غير موجود على هذا الجهاز، لذلك لم يزامنه وصل."),
        Some(OpenFolder),
        &[Folder],
        "The synced folder itself is gone or moved (contrast error.file_gone: one file inside it).",
    ),
    e(
        "error.name_refused",
        Forms::single(
            "Drive or this device won't accept the name of {file}. Skipped it; the rest of {folder} synced.",
        ),
        Forms::single("لا يقبل Drive أو هذا الجهاز اسم {file}. تم تخطيه ومزامنة بقية {folder}."),
        Some(OpenToRename),
        &[File, Folder],
        "The file name is too long, has a character or a reserved name that one side refuses. Wasl never renames it.",
    ),
    e(
        "error.sign_in_unreadable",
        Forms::single(
            "Wasl couldn't read your saved Google sign-in, so {folder} can't sync. Nothing was changed.",
        ),
        Forms::single(
            "تعذّر على وصل قراءة تسجيل دخولك المحفوظ إلى Google، لذلك لا يمكن مزامنة {folder}. لم يتغير شيء.",
        ),
        Some(SignInAgain),
        &[Folder],
        "The sign-in stored on this device is damaged or can't be opened (contrast error.auth: Google ended the session).",
    ),
    e(
        "error.invalid",
        Forms::single("Wasl couldn't do that: the request wasn't valid."),
        Forms::single("تعذّر على وصل تنفيذ ذلك: الطلب غير صالح."),
        None,
        &[],
        "Shown for a request the engine refused; Details carries the engine's own message.",
    ),
    e(
        "error.busy",
        Forms::single("Wasl is busy with that folder right now. Try again in a moment."),
        Forms::single("وصل مشغول بهذا المجلد الآن. حاول مجددًا بعد قليل."),
        Some(Retry),
        &[],
        "The folder is in the middle of a sync or another change.",
    ),
    e(
        "error.unsupported",
        Forms::single("This version of Wasl can't do that yet."),
        Forms::single("هذا الإصدار من وصل لا يستطيع تنفيذ ذلك بعد."),
        Some(UpdateWasl),
        &[],
        "The engine doesn't support the request (usually an older engine).",
    ),
    e(
        FALLBACK,
        Forms::single("Something went wrong syncing {folder}. Wasl will try again at the next sync."),
        Forms::single("حدث خطأ أثناء مزامنة {folder}. سيحاول وصل مجددًا في المزامنة التالية."),
        Some(Retry),
        &[Folder],
        "Fallback for any error the catalogue doesn't know. Always has Details; each unknown error is logged.",
    ),
    // ---- reaching the engine (the CLI and apps talk to a background program)
    e(
        "daemon.not_running",
        Forms::single("Wasl isn't running. Start it with: wasl daemon start"),
        Forms::single("وصل لا يعمل. شغّله بالأمر: wasl daemon start"),
        None,
        &[],
        "The command line couldn't find the background program. The command stays in English.",
    ),
    e(
        "daemon.untrusted",
        Forms::single("The program answering is not this user's Wasl."),
        Forms::single("البرنامج الذي يجيب ليس وصل الخاص بهذا المستخدم."),
        None,
        &[],
        "Whatever answers on Wasl's local address didn't prove it is this user's Wasl.",
    ),
    e(
        "daemon.version_mismatch",
        Forms::single(
            "The Wasl running in the background and this app don't speak the same version. Update Wasl.",
        ),
        Forms::single("وصل العامل في الخلفية وهذا التطبيق لا يستخدمان الإصدار نفسه. حدّث وصل."),
        Some(UpdateWasl),
        &[],
        "The background program and the app speak different protocol versions. Details has both numbers.",
    ),
    e(
        "daemon.connection_lost",
        Forms::single("Lost the connection to Wasl. Try again."),
        Forms::single("انقطع الاتصال بوصل. حاول مجددًا."),
        Some(Retry),
        &[],
        "The link to the background program broke in the middle of a request.",
    ),
];

/// The entry a coarse run error kind maps to.
pub fn key_for_kind(kind: crate::run::ErrorKind) -> &'static str {
    use crate::run::ErrorKind as K;
    match kind {
        K::Auth => "error.auth",
        K::Permission => "error.permission",
        K::Quota => "error.quota",
        K::NotFound => "error.not_found",
        K::Locked => "error.locked",
        K::Network => "error.network",
        K::RateLimit => "error.rate_limit",
        K::Server => "error.server",
        K::Io => "error.io",
        K::Integrity => "error.integrity",
        K::Other => FALLBACK,
    }
}

/// The most specific entry for a failure: the coarse `kind` and the raw cause it came with. The raw
/// text is only read here, to tell cases apart; it never reaches the words (only Details, redacted).
pub fn classify(kind: crate::run::ErrorKind, raw: &str) -> &'static str {
    use crate::run::ErrorKind as K;
    let r = raw.to_ascii_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|n| r.contains(n));
    let disk = has(&[
        "no space left",
        "disk full",
        "not enough space",
        "os error 28",
        "os error 112",
        "enospc",
        "storage is full",
    ]);
    let name = has(&[
        "name too long",
        "filename too long",
        "invalid name",
        "illegal character",
        "reserved name",
        "os error 123",
        "os error 206",
        "enametoolong",
    ]);
    match kind {
        K::Auth
            if has(&[
                "unreadable",
                "keyring",
                "decrypt",
                "damaged",
                "internal_failure",
                "could not be read",
            ]) =>
        {
            "error.sign_in_unreadable"
        }
        K::NotFound if has(&["folder", "directory", "root"]) && !has(&["file"]) => "error.folder_missing",
        K::NotFound => "error.file_gone",
        K::Quota if disk => "error.disk_full",
        K::Io | K::Other | K::Permission if disk => "error.disk_full",
        K::Io | K::Other | K::Permission | K::Server if name => "error.name_refused",
        K::Io if has(&["folder is missing", "directory not found", "no such directory"]) => {
            "error.folder_missing"
        }
        _ => key_for_kind(kind),
    }
}

/// The entry with this key.
pub fn entry(key: &str) -> Option<&'static Entry> {
    ENTRIES.iter().find(|e| e.key == key)
}

/// Words, fix and details for one entry in one language: what a screen draws and an agent reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rendered {
    pub key: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fix: Option<Fix>,
    #[serde(default, skip_serializing_if = "Values::is_empty")]
    pub values: Values,
    /// The raw cause, made safe by [`redact`]; shown collapsed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
}

impl Values {
    fn is_empty(&self) -> bool {
        *self == Values::default()
    }
}

fn stand_in(slot: Slot, lang: Lang) -> &'static str {
    match (slot, lang) {
        (Slot::Folder, Lang::En) => "this folder",
        (Slot::Folder, Lang::Ar) => "هذا المجلد",
        (Slot::File, Lang::En) => "a file",
        (Slot::File, Lang::Ar) => "ملف",
        (Slot::Device, Lang::En) => "another device",
        (Slot::Device, Lang::Ar) => "جهاز آخر",
        (Slot::Count, _) => "0",
    }
}

fn fill(template: &str, values: &Values, lang: Lang) -> String {
    let mut out = String::with_capacity(template.len() + 24);
    let mut rest = template;
    'scan: while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open..];
        for slot in [Slot::Folder, Slot::File, Slot::Device, Slot::Count] {
            let tag = format!("{{{}}}", slot.name());
            if after.starts_with(&tag) {
                let v = match slot {
                    Slot::Folder => values.folder.clone(),
                    Slot::File => values.file.clone(),
                    Slot::Device => values.device.clone(),
                    Slot::Count => values.count.map(|c| c.to_string()),
                };
                out.push_str(&v.unwrap_or_else(|| stand_in(slot, lang).to_owned()));
                rest = &after[tag.len()..];
                continue 'scan;
            }
        }
        out.push('{');
        rest = &after[1..];
    }
    out.push_str(rest);
    out
}

/// Renders `key` in `lang`. A key the catalogue doesn't have renders the fallback entry, keeps the
/// requested key out of the result's `key`, and calls `log` once with a line naming it, so it can be
/// added to the catalogue. `raw` is the underlying cause; it becomes Details after [`redact`].
pub fn render(
    key: &str,
    values: &Values,
    lang: Lang,
    raw: Option<&str>,
    log: &mut dyn FnMut(String),
) -> Rendered {
    let found = entry(key);
    if found.is_none() {
        log(format!(
            "wasl: no catalogue entry for error key {key:?}; showing the fallback"
        ));
    }
    let ent = found.unwrap_or_else(|| entry(FALLBACK).expect("the fallback entry exists"));
    let forms = match lang {
        Lang::En => &ent.en,
        Lang::Ar => &ent.ar,
    };
    let cat = values.count.map_or(Plural::Other, |n| plural(lang, n));
    let text = fill(forms.pick(cat), values, lang);
    let secrets: Vec<&str> = Vec::new();
    let details = raw
        .map(|r| redact(r, values.folder_root(), &secrets))
        .filter(|d| !d.is_empty());
    Rendered {
        key: ent.key.to_owned(),
        text,
        fix: ent.fix,
        values: values.clone(),
        details,
    }
}

impl Values {
    /// The folder's own path, when the caller put one in `folder`; `redact` shortens paths to it.
    fn folder_root(&self) -> Option<&str> {
        self.folder
            .as_deref()
            .filter(|f| f.contains('/') || f.contains('\\'))
    }
}

/// Makes a raw cause safe to show and to copy: tokens, sign-in codes, e-mail addresses and the
/// `secrets` given (account ids and the like) become `[hidden]`; a path inside `folder_root` is cut
/// to `Folder/…/file`; any other absolute path becomes `…`.
pub fn redact(raw: &str, folder_root: Option<&str>, secrets: &[&str]) -> String {
    let mut text = raw.to_owned();
    for s in secrets.iter().filter(|s| !s.is_empty()) {
        text = text.replace(s, "[hidden]");
    }
    let root = folder_root
        .map(|r| r.trim_end_matches(['/', '\\']).to_owned())
        .filter(|r| !r.is_empty());
    let root_name = root
        .as_deref()
        .map(|r| r.rsplit(['/', '\\']).next().unwrap_or(r).to_owned());
    let root_mark = "\u{2}ROOT\u{2}";
    if let Some(r) = &root {
        for variant in [r.clone(), r.replace('\\', "/"), r.replace('/', "\\")] {
            text = text.replace(&variant, root_mark);
        }
    }
    let mut out = String::with_capacity(text.len());
    let mut hide_next = false;
    for piece in split_keep(&text) {
        match piece {
            Piece::Space(s) => out.push_str(s),
            Piece::Word(w) => {
                let (lead, core, trail) = peel(w);
                if hide_next && !core.is_empty() {
                    hide_next = false;
                    out.push_str(lead);
                    out.push_str("[hidden]");
                    out.push_str(trail);
                    continue;
                }
                if core.eq_ignore_ascii_case("bearer") || core.eq_ignore_ascii_case("authorization:") {
                    hide_next = true;
                }
                out.push_str(lead);
                out.push_str(&redact_word(core, root_name.as_deref(), root_mark));
                out.push_str(trail);
            }
        }
    }
    out
}

enum Piece<'a> {
    Space(&'a str),
    Word(&'a str),
}

fn split_keep(s: &str) -> Vec<Piece<'_>> {
    let mut v = Vec::new();
    let mut start = 0;
    let mut in_space = None;
    for (i, c) in s.char_indices() {
        let sp = c.is_whitespace();
        match in_space {
            None => in_space = Some(sp),
            Some(prev) if prev != sp => {
                v.push(if prev {
                    Piece::Space(&s[start..i])
                } else {
                    Piece::Word(&s[start..i])
                });
                start = i;
                in_space = Some(sp);
            }
            _ => {}
        }
    }
    if let Some(prev) = in_space {
        v.push(if prev {
            Piece::Space(&s[start..])
        } else {
            Piece::Word(&s[start..])
        });
    }
    v
}

/// Splits quotes and brackets off the ends of a word.
fn peel(w: &str) -> (&str, &str, &str) {
    let lead_len = w.len() - w.trim_start_matches(['\'', '"', '(', '[', '<']).len();
    let rest = &w[lead_len..];
    let trail_len = rest.len() - rest.trim_end_matches(['\'', '"', ')', ']', '>', ',', ';']).len();
    (
        &w[..lead_len],
        &rest[..rest.len() - trail_len],
        &rest[rest.len() - trail_len..],
    )
}

fn redact_word(w: &str, root_name: Option<&str>, root_mark: &str) -> String {
    // key=value and key:value for secrets.
    for sep in ['=', ':'] {
        if let Some((k, _)) = w.split_once(sep) {
            let k = k.trim_matches(['"', '\'']).to_ascii_lowercase();
            if matches!(
                k.as_str(),
                "access_token"
                    | "refresh_token"
                    | "id_token"
                    | "token"
                    | "client_secret"
                    | "password"
                    | "passphrase"
                    | "code"
                    | "api_key"
                    | "key"
                    | "secret"
                    | "authorization"
                    | "account"
                    | "account_id"
                    | "user"
                    | "email"
            ) && !w.ends_with(sep)
            {
                return format!("{}{sep}[hidden]", &w[..w.find(sep).unwrap_or(0)]);
            }
        }
    }
    let lower = w.to_ascii_lowercase();
    if lower.starts_with("ya29.")
        || w.starts_with("1//")
        || w.starts_with("eyJ")
        || lower.starts_with("gho_")
        || lower.starts_with("ghp_")
        || lower.starts_with("sk-")
        || lower.starts_with("wasl-token")
    {
        return "[hidden]".to_owned();
    }
    if w.contains('@') && w.contains('.') && !w.starts_with('/') && !w.contains('/') {
        return "[hidden]".to_owned();
    }
    if let Some(pos) = w.find(root_mark) {
        let after = w[pos + root_mark.len()..].replace('\\', "/");
        let parts: Vec<&str> = after.split('/').filter(|p| !p.is_empty()).collect();
        let name = root_name.unwrap_or("…");
        return match parts.as_slice() {
            [] => name.to_owned(),
            [one] => format!("{name}/{one}"),
            [.., last] => format!("{name}/…/{last}"),
        };
    }
    if looks_absolute(w) {
        return "…".to_owned();
    }
    w.to_owned()
}

fn looks_absolute(w: &str) -> bool {
    let b = w.as_bytes();
    (w.starts_with('/') && w.len() > 1 && !w.starts_with("//") || w.starts_with("//") && w.len() > 2)
        || w.starts_with("\\\\")
        || (b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/'))
        || w.starts_with("~/")
        || w.starts_with("file://")
}

/// The reference document: one section per entry, generated from [`ENTRIES`].
pub fn reference_markdown() -> String {
    let mut s = String::from(
        "# Status and error words\n\n\
         Generated from `engine/crates/wasl-proto/src/words.rs`; do not edit by hand. Run\n\
         `WASL_BLESS=1 cargo test -p wasl-proto words` after changing the catalogue.\n\n\
         Every status and error Wasl shows is one of these entries. The key is stable: a script or an\n\
         agent acts on the key, the typed values and the fix, never on the sentence. Values in the words\n\
         are written `{name}`; the engine fills them and renders the entry in the caller's language,\n\
         falling back to English.\n\n",
    );
    for ent in ENTRIES {
        s.push_str(&format!("## `{}`\n\n", ent.key));
        s.push_str(&format!("- Note: {}\n", ent.note));
        let slots: Vec<&str> = ent.slots.iter().map(|x| x.name()).collect();
        s.push_str(&format!(
            "- Values: {}\n",
            if slots.is_empty() {
                "none".into()
            } else {
                slots.join(", ")
            }
        ));
        s.push_str(&format!(
            "- Fix: {}\n",
            ent.fix
                .map_or("none".to_owned(), |f| format!("`{}`", fix_name(f)))
        ));
        for (lang, forms) in [("English", &ent.en), ("Arabic", &ent.ar)] {
            for (cat, text) in [
                ("zero", forms.zero),
                ("one", forms.one),
                ("two", forms.two),
                ("few", forms.few),
                ("many", forms.many),
                ("other", Some(forms.other)),
            ] {
                if let Some(t) = text {
                    if ent.slots.contains(&Slot::Count) {
                        s.push_str(&format!("- {lang} ({cat}): {t}\n"));
                    } else {
                        s.push_str(&format!("- {lang}: {t}\n"));
                    }
                }
            }
        }
        s.push('\n');
    }
    s
}

fn fix_name(f: Fix) -> String {
    serde_json::to_value(f)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quiet() -> impl FnMut(String) {
        |_| {}
    }

    #[test]
    fn every_entry_is_complete() {
        let mut seen = std::collections::HashSet::new();
        for ent in ENTRIES {
            assert!(seen.insert(ent.key), "duplicate key {}", ent.key);
            assert!(
                !ent.note.trim().is_empty(),
                "{} has no translator's note",
                ent.key
            );
            for (lang, forms) in [("en", &ent.en), ("ar", &ent.ar)] {
                assert!(!forms.other.trim().is_empty(), "{} lacks {lang} text", ent.key);
                for t in forms.all() {
                    assert!(!t.trim().is_empty(), "{} has an empty {lang} form", ent.key);
                    for slot in [Slot::Folder, Slot::File, Slot::Device, Slot::Count] {
                        if t.contains(&format!("{{{}}}", slot.name())) {
                            assert!(
                                ent.slots.contains(&slot),
                                "{} {lang} uses {{{}}} without declaring it",
                                ent.key,
                                slot.name()
                            );
                        }
                    }
                }
            }
            let plural_entry = ent.slots.contains(&Slot::Count);
            if plural_entry {
                assert!(ent.en.one.is_some(), "{} needs an English 'one' form", ent.key);
                let a = &ent.ar;
                assert!(
                    a.zero.is_some()
                        && a.one.is_some()
                        && a.two.is_some()
                        && a.few.is_some()
                        && a.many.is_some(),
                    "{} needs all six Arabic forms",
                    ent.key
                );
            } else {
                let forms = [&ent.en, &ent.ar];
                assert!(
                    forms.iter().all(|f| f.zero.is_none()
                        && f.one.is_none()
                        && f.two.is_none()
                        && f.few.is_none()
                        && f.many.is_none()),
                    "{} has plural forms but takes no count",
                    ent.key
                );
            }
        }
    }

    #[test]
    fn plurals_in_english_and_arabic() {
        let r = |n, lang| {
            render(
                "status.folders_need_you",
                &Values::default().count(n),
                lang,
                None,
                &mut quiet(),
            )
            .text
        };
        assert_eq!(r(1, Lang::En), "1 folder needs you");
        assert_eq!(r(2, Lang::En), "2 folders need you");
        assert_eq!(r(0, Lang::Ar), "لا مجلدات تحتاج إليك");
        assert_eq!(r(1, Lang::Ar), "مجلد واحد يحتاج إليك");
        assert_eq!(r(2, Lang::Ar), "مجلدان يحتاجان إليك");
        assert_eq!(r(3, Lang::Ar), "3 مجلدات تحتاج إليك");
        assert_eq!(r(11, Lang::Ar), "11 مجلدًا يحتاج إليك");
        assert_eq!(r(100, Lang::Ar), "100 مجلد يحتاج إليك");
        assert_eq!(r(102, Lang::Ar), "102 مجلد يحتاج إليك");
    }

    #[test]
    fn unknown_key_gets_the_fallback_and_one_log_line() {
        let mut lines = Vec::new();
        let r = render(
            "error.never_heard_of_it",
            &Values::default().folder("LifeOS"),
            Lang::En,
            Some("boom"),
            &mut |l| lines.push(l),
        );
        assert_eq!(r.key, FALLBACK);
        assert_eq!(
            r.text,
            "Something went wrong syncing LifeOS. Wasl will try again at the next sync."
        );
        assert_eq!(r.fix, Some(Fix::Retry));
        assert_eq!(r.details.as_deref(), Some("boom"));
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("error.never_heard_of_it"));
        let mut again = Vec::new();
        render("error.network", &Values::default(), Lang::En, None, &mut |l| {
            again.push(l)
        });
        assert!(again.is_empty(), "a known key logs nothing");
    }

    #[test]
    fn a_missing_value_reads_plainly_and_a_value_is_not_filled_twice() {
        let r = render(
            "error.not_found",
            &Values::default(),
            Lang::En,
            None,
            &mut quiet(),
        );
        assert_eq!(
            r.text,
            "a file isn't there any more. Skipped it; the rest of this folder synced."
        );
        let r = render(
            "error.not_found",
            &Values::default().file("{folder}").folder("Docs"),
            Lang::En,
            None,
            &mut quiet(),
        );
        assert_eq!(
            r.text,
            "{folder} isn't there any more. Skipped it; the rest of Docs synced."
        );
    }

    #[test]
    fn every_run_error_kind_has_its_entry() {
        use crate::run::ErrorKind as K;
        let want = [
            (K::Auth, "error.auth"),
            (K::Permission, "error.permission"),
            (K::Quota, "error.quota"),
            (K::NotFound, "error.not_found"),
            (K::Locked, "error.locked"),
            (K::Network, "error.network"),
            (K::RateLimit, "error.rate_limit"),
            (K::Server, "error.server"),
            (K::Io, "error.io"),
            (K::Integrity, "error.integrity"),
            (K::Other, FALLBACK),
        ];
        for (kind, key) in want {
            assert_eq!(key_for_kind(kind), key);
            assert!(entry(key).is_some(), "{key} is not in the catalogue");
        }
    }

    #[test]
    fn the_failures_people_hit_each_get_their_own_entry() {
        use crate::run::ErrorKind as K;
        let cases = [
            (K::NotFound, "file not found: report.xlsx", "error.file_gone"),
            (
                K::NotFound,
                "local folder not found: /home/a/LifeOS",
                "error.folder_missing",
            ),
            (K::Io, "No space left on device (os error 28)", "error.disk_full"),
            (
                K::Quota,
                "storageQuotaExceeded: the user's Drive storage quota has been exceeded",
                "error.quota",
            ),
            (
                K::Quota,
                "write failed: not enough space on the disk",
                "error.disk_full",
            ),
            (
                K::Io,
                "The filename or extension is too long. (os error 206)",
                "error.name_refused",
            ),
            (K::Server, "invalid name: reserved name CON", "error.name_refused"),
            (
                K::Auth,
                "invalid_grant: token has been expired or revoked",
                "error.auth",
            ),
            (
                K::Auth,
                "internal_failure: saved sign-in could not be read from the keyring",
                "error.sign_in_unreadable",
            ),
            (K::Network, "dns error", "error.network"),
        ];
        for (kind, raw, key) in cases {
            assert_eq!(classify(kind, raw), key, "{raw}");
            assert!(entry(key).is_some());
        }
        let fix = |k| entry(k).unwrap().fix;
        assert_eq!(fix("error.disk_full"), Some(Fix::OpenStorageSettings));
        assert_eq!(fix("error.quota"), Some(Fix::OpenDriveStorage));
        assert_eq!(fix("error.name_refused"), Some(Fix::OpenToRename));
        assert_eq!(fix("error.sign_in_unreadable"), Some(Fix::SignInAgain));
        let gone = render(
            "error.file_gone",
            &Values::default().file("a.txt").folder("Docs"),
            Lang::En,
            None,
            &mut quiet(),
        );
        assert_eq!(
            gone.text,
            "a.txt was gone before Wasl could sync it. Skipped it; the rest of Docs synced."
        );
    }

    #[test]
    fn no_entry_shows_a_bare_internal_code() {
        for ent in ENTRIES {
            for t in ent.en.all().chain(ent.ar.all()) {
                let bare = t
                    .split(|c: char| c.is_whitespace() || "{}.,:'\"".contains(c))
                    .any(|w| w.contains('_') || (w.len() > 2 && w.chars().all(|c| c.is_ascii_uppercase())));
                assert!(!bare, "{} shows a bare code: {t}", ent.key);
            }
        }
    }

    #[test]
    fn missing_language_falls_back_to_english() {
        assert_eq!(Lang::parse("fr_FR"), None);
        assert_eq!(Lang::parse("ar-EG"), Some(Lang::Ar));
    }

    #[test]
    fn details_never_hold_tokens_accounts_or_outside_paths() {
        let raw = "request failed for /home/ahmad/Secret/other.txt with Bearer ya29.A0planted-token \
                   access_token=abc123 refresh_token=1//planted for ahmad@example.com account acct-7f3 \
                   at C:\\Users\\ahmad\\Documents\\LifeOS\\sub\\deep\\file.xlsx and '/home/ahmad/LifeOS/a/b.txt'";
        let d = redact(raw, Some("/home/ahmad/LifeOS"), &["acct-7f3"]);
        for planted in [
            "ya29",
            "abc123",
            "1//planted",
            "ahmad@example.com",
            "acct-7f3",
            "/home/ahmad/Secret",
            "C:\\Users",
            "Documents",
        ] {
            assert!(!d.contains(planted), "{planted} leaked: {d}");
        }
        assert!(d.contains("LifeOS/…/b.txt"), "{d}");
        assert!(d.starts_with("request failed for … with Bearer [hidden]"), "{d}");
    }

    #[test]
    fn a_path_in_the_folder_is_cut_to_the_folder() {
        let d = redact(
            "cannot open C:\\Users\\a b\\LifeOS\\x\\y\\z.txt: denied",
            Some("C:\\Users\\a b\\LifeOS"),
            &[],
        );
        assert_eq!(d, "cannot open LifeOS/…/z.txt: denied");
    }

    #[test]
    fn reference_doc_is_current() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../docs/STATUS_WORDS.md");
        let want = reference_markdown();
        if std::env::var_os("WASL_BLESS").is_some() {
            std::fs::write(&path, &want).unwrap();
        }
        let have = std::fs::read_to_string(&path).unwrap_or_default();
        assert_eq!(
            have.replace("\r\n", "\n"),
            want,
            "docs/STATUS_WORDS.md is stale: WASL_BLESS=1 cargo test -p wasl-proto words"
        );
    }
}
