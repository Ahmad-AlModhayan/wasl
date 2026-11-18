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
        Forms::single(
            "Wasl isn't allowed to use {folder} any more, so it stopped syncing there. Nothing was changed.",
        ),
        Forms::single("لم يعد وصل مسموحًا له باستخدام {folder}، لذلك توقفت المزامنة هناك. لم يتغير شيء."),
        Some(OpenFolder),
        &[Folder],
        "The folder or the account refused Wasl access; the run stops (a fatal error). {folder} is the synced folder.",
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
        Forms::single("Wasl couldn't find something it needed for {folder}. Nothing was changed."),
        Forms::single("تعذّر على وصل العثور على شيء يحتاجه لـ {folder}. لم يتغير شيء."),
        Some(Retry),
        &[Folder],
        "Something the run needed (a Drive folder, an upload) is gone. Not one vanished file: that is error.file_gone.",
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
        Forms::single(
            "Wasl couldn't read or write a file in {folder}. It will be tried again at the next sync.",
        ),
        Forms::single(
            "تعذّر على وصل قراءة ملف في {folder} أو الكتابة فيه. ستتم محاولته مجددًا في المزامنة التالية.",
        ),
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

/// The most specific entry for a failure that came as a coarse `kind` and its raw cause, for callers
/// that hold nothing more typed (the CLI and apps read the daemon's reply). It only trusts the numbers
/// the operating system itself puts in an I/O message (`(os error 28)`), never words, so a file called
/// "Disk full notes.txt" can't change the answer.
pub fn classify(kind: crate::run::ErrorKind, raw: &str) -> &'static str {
    use crate::run::ErrorKind as K;
    let code = os_error_code(raw);
    match (kind, code) {
        (K::Io | K::Quota | K::Other, Some(28 | 112 | 39 | 122)) => "error.disk_full",
        (K::Io | K::Other | K::Permission, Some(36 | 63 | 123 | 206)) => "error.name_refused",
        _ => key_for_kind(kind),
    }
}

/// The number in a trailing `(os error N)`, as `std::io::Error` prints it.
pub fn os_error_code(raw: &str) -> Option<i32> {
    let i = raw.rfind("(os error ")?;
    let rest = &raw[i + "(os error ".len()..];
    let end = rest.find(')')?;
    rest[..end].parse().ok()
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
    secrets: &[&str],
    log: &mut dyn FnMut(String),
) -> Rendered {
    let ent = entry(key).unwrap_or_else(|| entry(FALLBACK).expect("the fallback entry exists"));
    if ent.key == FALLBACK {
        log(format!(
            "wasl: no specific catalogue entry for error {key:?}; showing the fallback"
        ));
    }
    let forms = match lang {
        Lang::En => &ent.en,
        Lang::Ar => &ent.ar,
    };
    let cat = values.count.map_or(Plural::Other, |n| plural(lang, n));
    let text = fill(forms.pick(cat), values, lang);
    let details = raw
        .map(|r| redact(r, values.folder_root(), secrets))
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

/// Makes a raw cause safe to show and to copy. Tokens, sign-in codes, `Bearer`/`Basic` credentials,
/// secret-looking `key=value` and JSON pairs, e-mail addresses and every one of `secrets` (account ids
/// and the like) become `[hidden]`. A path inside `folder_root` is cut to `Folder/…/file`; any other
/// absolute path becomes `…`, spaces in it included. The whole text is scanned, not word by word.
pub fn redact(raw: &str, folder_root: Option<&str>, secrets: &[&str]) -> String {
    use regex::{Captures, Regex};
    use std::sync::OnceLock;
    struct Rules {
        credential: Regex,
        pair: Regex,
        token: Regex,
        email: Regex,
        marked: Regex,
        windows_path: Regex,
        unix_path: Regex,
    }
    static RULES: OnceLock<Rules> = OnceLock::new();
    let r = RULES.get_or_init(|| {
        // A path runs to the next whitespace; a space stays inside it only when the next piece holds
        // a separator too (`Jane Doe\x.txt`), so words after a path ("isn't in a view") stay.
        let chunk = r#"[^\s'"<>|*?]*"#;
        let tail = format!(r#"{chunk}(?: {chunk}[\\/]{chunk})*"#);
        let re = |p: &str| Regex::new(p).expect("a fixed pattern");
        Rules {
            credential: re(r"(?i)\b(bearer|basic)\s+[A-Za-z0-9._~+/=-]+"),
            pair: re(
                r#"(?i)\b("?(?:access_token|refresh_token|id_token|token|client_secret|password|passphrase|code|api_key|secret|authorization|account_id|email|user)"?\s*[:=]\s*)("[^"]*"|'[^']*'|[^\s&,;}]+)"#,
            ),
            token: re(r"(?i)(?:ya29\.|1//|eyJ|gho_|ghp_|sk-|GOCSPX-)[A-Za-z0-9._\-/]+"),
            email: re(r"[\w.+-]+@[\w-]+(?:\.[\w-]+)+"),
            marked: re(&format!("\u{2}ROOT\u{2}({tail})")),
            windows_path: re(&format!(r#"[A-Za-z]:[\\/]{tail}"#)),
            unix_path: re(&format!(r#"(^|[\s'"(=])(/[^\s/]{tail})"#)),
        }
    });
    let mut text = raw.to_owned();
    for s in secrets.iter().filter(|s| !s.is_empty()) {
        text = text.replace(s, "[hidden]");
    }
    text = r.credential.replace_all(&text, "$1 [hidden]").into_owned();
    text = r.pair.replace_all(&text, "${1}[hidden]").into_owned();
    text = r.token.replace_all(&text, "[hidden]").into_owned();
    text = r.email.replace_all(&text, "[hidden]").into_owned();
    let root = folder_root
        .map(|f| f.trim_end_matches(['/', '\\']))
        .filter(|f| !f.is_empty());
    let root_name = root.map(|f| f.rsplit(['/', '\\']).next().unwrap_or(f).to_owned());
    if let Some(root) = root {
        let variants: Vec<String> = [root.to_owned(), root.replace('\\', "/"), root.replace('/', "\\")]
            .iter()
            .map(|v| regex::escape(v))
            .collect();
        // The root counts only as a whole name: `/a/Life` is not the start of `/a/LifeOS2/x`.
        let inside =
            Regex::new(&format!(r"(?:{})([\\/]|[^\w\\/.-]|$)", variants.join("|"))).expect("a built pattern");
        text = inside.replace_all(&text, "\u{2}ROOT\u{2}$1").into_owned();
    }
    let name = root_name.unwrap_or_else(|| "…".to_owned());
    text = r
        .marked
        .replace_all(&text, |c: &Captures| {
            let (body, rest) = split_punct(&c[1]);
            let parts: Vec<&str> = body.split(['/', '\\']).filter(|p| !p.is_empty()).collect();
            let short = match parts.as_slice() {
                [] => name.clone(),
                [one] => format!("{name}/{one}"),
                [.., last] => format!("{name}/…/{last}"),
            };
            format!("{short}{rest}")
        })
        .into_owned();
    text = r
        .windows_path
        .replace_all(&text, |c: &Captures| format!("…{}", split_punct(&c[0]).1))
        .into_owned();
    text = r
        .unix_path
        .replace_all(&text, |c: &Captures| {
            format!("{}…{}", &c[1], split_punct(&c[2]).1)
        })
        .into_owned();
    text
}

/// Splits trailing sentence punctuation off a matched path: `x.txt:` is `x.txt` and `:`.
fn split_punct(path: &str) -> (&str, &str) {
    let cut = path.trim_end_matches([':', ',', ';', '.', ')']).len();
    path.split_at(cut)
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
                &[],
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
    fn an_unknown_key_or_an_other_error_gets_the_fallback_and_one_log_line() {
        let mut lines = Vec::new();
        let r = render(
            "error.never_heard_of_it",
            &Values::default().folder("LifeOS"),
            Lang::En,
            Some("boom"),
            &[],
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
        // An error kind the engine itself calls Other reaches the fallback by key, and logs too.
        let mut other = Vec::new();
        let r = render(
            key_for_kind(crate::run::ErrorKind::Other),
            &Values::default(),
            Lang::En,
            None,
            &[],
            &mut |l| other.push(l),
        );
        assert_eq!(r.key, FALLBACK);
        assert_eq!(other.len(), 1);
        let mut known = Vec::new();
        render(
            "error.network",
            &Values::default(),
            Lang::En,
            None,
            &[],
            &mut |l| known.push(l),
        );
        assert!(known.is_empty(), "a known key logs nothing");
    }

    #[test]
    fn a_missing_value_reads_plainly_and_a_value_is_not_filled_twice() {
        let r = render(
            "error.file_gone",
            &Values::default(),
            Lang::En,
            None,
            &[],
            &mut quiet(),
        );
        assert_eq!(
            r.text,
            "a file was gone before Wasl could sync it. Skipped it; the rest of this folder synced."
        );
        let r = render(
            "error.file_gone",
            &Values::default().file("{folder}").folder("Docs"),
            Lang::En,
            None,
            &[],
            &mut quiet(),
        );
        assert_eq!(
            r.text,
            "{folder} was gone before Wasl could sync it. Skipped it; the rest of Docs synced."
        );
    }

    #[test]
    fn only_the_operating_systems_error_numbers_pick_the_specific_entries() {
        use crate::run::ErrorKind as K;
        let cases = [
            (K::Io, "No space left on device (os error 28)", "error.disk_full"),
            (
                K::Io,
                "There is not enough space on the disk. (os error 112)",
                "error.disk_full",
            ),
            (
                K::Io,
                "The filename or extension is too long. (os error 206)",
                "error.name_refused",
            ),
            (K::Io, "File name too long (os error 36)", "error.name_refused"),
            (
                K::Quota,
                "storageQuotaExceeded: the user's Drive storage quota has been exceeded",
                "error.quota",
            ),
            (
                K::Permission,
                "Disk full notes.txt: access denied (os error 5)",
                "error.permission",
            ),
            (K::Io, "cannot read Folder plan.docx (os error 5)", "error.io"),
            (K::Io, "boom (os error 281)", "error.io"),
            (
                K::Auth,
                "invalid_grant: token has been expired or revoked",
                "error.auth",
            ),
            (K::NotFound, "This pair has no Drive folder", "error.not_found"),
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
        let gone = render(
            "error.file_gone",
            &Values::default().file("a.txt").folder("Docs"),
            Lang::En,
            None,
            &[],
            &mut quiet(),
        );
        assert_eq!(
            gone.text,
            "a.txt was gone before Wasl could sync it. Skipped it; the rest of Docs synced."
        );
    }

    #[test]
    fn a_failure_that_stops_the_run_never_says_the_rest_synced() {
        // The engine stops the whole run for Auth, Permission and Quota (EngineError::is_fatal).
        for key in [
            "error.auth",
            "error.permission",
            "error.quota",
            "error.not_found",
            "error.io",
        ] {
            let ent = entry(key).unwrap();
            for t in ent.en.all().chain(ent.ar.all()) {
                assert!(
                    !t.contains("the rest") && !t.contains("بقية") && !t.contains("Skipped"),
                    "{key}: {t}"
                );
            }
        }
    }

    fn planted(raw: &str, root: Option<&str>) -> String {
        redact(raw, root, &["acct-7f3"])
    }

    #[test]
    fn details_never_hold_tokens_accounts_or_outside_paths() {
        let cases = [
            "request failed for Bearer ya29.A0planted-token on acct-7f3",
            r#"{"access_token":"ya29.abc","refresh_token":"opaque-refresh-xyz","client_secret":"GOCSPX-planted"}"#,
            "GET /cb?code=4/0AplantedCode&state=x failed",
            "Authorization: Basic dXNlcjpwbGFudGVkcGFzcw==",
            "sent to user@example.com",
            "failed to open C:\\Users\\Jane Doe\\Documents\\x.txt: denied",
            "cannot read /home/user/Secret Stuff/other.txt (os error 5)",
            "path=/home/user/Secret/other.txt",
            "key=acct-7f3",
        ];
        for raw in cases {
            let d = planted(raw, None);
            for leak in [
                "ya29",
                "opaque-refresh",
                "GOCSPX",
                "4/0Aplanted",
                "dXNlcjp",
                "user@example.com",
                "Jane",
                "Doe",
                "Secret",
                "acct-7f3",
                "C:\\Users",
                "/home/user",
            ] {
                assert!(!d.contains(leak), "{leak} leaked from {raw:?}: {d}");
            }
        }
        assert_eq!(
            planted("failed to open C:\\Users\\Jane Doe\\x.txt: denied", None),
            "failed to open …: denied"
        );
    }

    #[test]
    fn a_path_in_the_folder_is_cut_to_the_folder() {
        let d = redact(
            "cannot open C:\\Users\\a b\\LifeOS\\x\\y\\z.txt: denied",
            Some("C:\\Users\\a b\\LifeOS"),
            &[],
        );
        assert_eq!(d, "cannot open LifeOS/…/z.txt: denied");
        let d = redact(
            "bad '/home/user/LifeOS/a/b.txt' and /home/user/Other/c.txt (os error 2)",
            Some("/home/user/LifeOS"),
            &[],
        );
        assert_eq!(d, "bad 'LifeOS/…/b.txt' and … (os error 2)");
        // A sibling that only starts with the folder's name is outside the folder.
        let d = redact(
            "cannot open /home/user/LifeOS2/x/y.txt: denied",
            Some("/home/user/LifeOS"),
            &[],
        );
        assert_eq!(d, "cannot open …: denied");
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
